//! Text-based map loading.
//!
//! Maps are authored as a plain grid of characters in `assets/maps/*.txt`, one
//! character per tile (see [`Tile::from_char`] for the legend). The grid is
//! parsed into a [`TileMap`], which is then rendered on entering the game.
//!
//! Walls are *auto-tiled*: each wall cell picks a sprite based on which of its
//! four orthogonal neighbours are also walls, so a hand-drawn line of `w`s turns
//! into proper end caps, corners and edges without any extra annotation.

use bevy::prelude::*;

use super::music::Song;
#[cfg(feature = "client")]
use super::state::GameState;

/// Side length, in world units, of a single map tile. All tile art is 64x64.
pub const TILE_SIZE: f32 = 64.0;

/// Path to the map that is loaded at startup, relative to the working directory.
const MAP_PATH: &str = "assets/maps/arena.txt";

/// Built-in fallback used when [`MAP_PATH`] cannot be read, so the game always
/// runs even if the file is missing. Keep it in sync with `assets/maps/arena.txt`.
const DEFAULT_MAP: &str = "\
wwwwwwwwwwwwwwww
wsxxxxxxxxxxxxsw
wxxxxx1xx2xxxxxw
wxxwwxxxxxxwwxxw
wxxwwxxxxxxwwxxw
wxxxxxxsxsxxxxxw
wxxxxxxxxxxxxxxw
wxxwwxx1xxxwwxxw
wxxwwxxxx2xwwxxw
wsxxxxxxxxxxxxsw
wwwwwwwwwwwwwwww
";

/// A single cell of the map.
///
/// New gameplay tiles (table, bush, chair, pickup spawn, ...) get added here and
/// in [`Tile::from_char`]; the rest of the pipeline only needs to know whether a
/// tile draws floor underneath it and whether it counts as a wall for autotiling.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tile {
    /// Empty space: nothing is rendered.
    #[default]
    Void,
    /// Plain walkable floor.
    Floor,
    /// Solid wall; auto-tiled from its neighbours.
    Wall,
    /// Walkable floor that is also a player spawn location.
    Spawn,
    /// Decorative box object (non-interactive for now); draws floor underneath.
    Box1,
    /// Second decorative box variant; draws floor underneath.
    Box2,
}

impl Tile {
    /// Maps a map-file character to a tile. Case-insensitive; unknown characters
    /// (including spaces and `.`) are treated as [`Tile::Void`].
    fn from_char(c: char) -> Tile {
        match c.to_ascii_lowercase() {
            'x' => Tile::Floor,
            'w' => Tile::Wall,
            's' => Tile::Spawn,
            '1' => Tile::Box1,
            '2' => Tile::Box2,
            _ => Tile::Void,
        }
    }

    /// Whether a floor sprite should be drawn beneath this tile. Walls are drawn
    /// on top of floor so that the transparent margins of bar/corner art blend in.
    #[cfg(feature = "client")]
    fn draws_floor(self) -> bool {
        !matches!(self, Tile::Void)
    }

    #[cfg(feature = "client")]
    fn is_wall(self) -> bool {
        matches!(self, Tile::Wall)
    }

    /// Asset path for a sprite drawn on top of the floor for this tile, if any.
    /// Extension point for future object tiles (table, bush, ...).
    #[cfg(feature = "client")]
    fn object_sprite(self) -> Option<&'static str> {
        match self {
            Tile::Box1 => Some("box-1.png"),
            Tile::Box2 => Some("box-2.png"),
            _ => None,
        }
    }
}

/// A parsed map: a `width * height` grid of [`Tile`]s plus precomputed spawn
/// locations in world space. The grid is laid out centred on the world origin.
#[derive(Clone, Debug)]
pub struct TileMap {
    width: usize,
    height: usize,
    tiles: Vec<Tile>,
    spawns: Vec<Vec2>,
    song: Song,
}

impl TileMap {
    /// Parses a map from text. Blank lines and `#` comments are ignored. A line
    /// of the form `key: value` is a directive (currently only `song:`); every
    /// other line is one grid row, padded on the right with [`Tile::Void`] to the
    /// width of the widest row. Grid rows only contain tile characters, so the
    /// presence of a `:` unambiguously marks a directive.
    pub fn parse(text: &str) -> TileMap {
        let mut song = Song::default();
        let rows: Vec<&str> = text
            .lines()
            .filter(|line| !line.is_empty() && !line.trim_start().starts_with('#'))
            .filter(|line| match line.split_once(':') {
                Some((key, value)) => {
                    match key.trim() {
                        "song" => match Song::from_name(value) {
                            Some(s) => song = s,
                            None => {
                                warn!("unknown song `{}` in map; using default", value.trim())
                            }
                        },
                        other => warn!("unknown map directive `{other}`; ignoring"),
                    }
                    false // directive line, not a grid row
                }
                None => true,
            })
            .collect();

        let width = rows
            .iter()
            .map(|row| row.chars().count())
            .max()
            .unwrap_or(0);
        let height = rows.len();

        let mut tiles = vec![Tile::Void; width * height];
        for (row, line) in rows.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                tiles[row * width + col] = Tile::from_char(ch);
            }
        }

        let mut map = TileMap {
            width,
            height,
            tiles,
            spawns: Vec::new(),
            song,
        };

        // Precompute spawn points in world space now that the size is known.
        for row in 0..height {
            for col in 0..width {
                if map.tile_at(col as i32, row as i32) == Tile::Spawn {
                    map.spawns.push(map.cell_center(col, row));
                }
            }
        }

        map
    }

    /// Total size of the map in world units.
    pub fn world_size(&self) -> Vec2 {
        Vec2::new(
            self.width as f32 * TILE_SIZE,
            self.height as f32 * TILE_SIZE,
        )
    }

    /// World-space bounds of the map, centred on the origin.
    pub fn bounds(&self) -> ArenaBounds {
        let half = self.world_size() / 2.0;
        ArenaBounds {
            min: -half,
            max: half,
        }
    }

    /// World-space player spawn points, in reading order (top-left first).
    pub fn spawn_points(&self) -> &[Vec2] {
        &self.spawns
    }

    /// The background track this map requests, defaulting to [`Song::ShooterLoop`]
    /// when the map file has no `song:` directive.
    pub fn song(&self) -> Song {
        self.song
    }

    /// World-space centre of cell `(col, row)`. Row 0 is the top of the file,
    /// which maps to the top (`+y`) of the world.
    fn cell_center(&self, col: usize, row: usize) -> Vec2 {
        let size = self.world_size();
        Vec2::new(
            -size.x / 2.0 + (col as f32 + 0.5) * TILE_SIZE,
            size.y / 2.0 - (row as f32 + 0.5) * TILE_SIZE,
        )
    }

    /// Tile at signed coordinates; anything out of bounds reads as [`Tile::Void`]
    /// so that walls on the map edge cap themselves correctly.
    fn tile_at(&self, col: i32, row: i32) -> Tile {
        if col < 0 || row < 0 || col >= self.width as i32 || row >= self.height as i32 {
            Tile::Void
        } else {
            self.tiles[row as usize * self.width + col as usize]
        }
    }

    #[cfg(feature = "client")]
    fn is_wall_at(&self, col: i32, row: i32) -> bool {
        self.tile_at(col, row).is_wall()
    }

    /// Converts a world-space position into the grid cell that contains it.
    fn world_to_cell(&self, pos: Vec2) -> (i32, i32) {
        let size = self.world_size();
        let col = ((pos.x + size.x / 2.0) / TILE_SIZE).floor() as i32;
        let row = ((size.y / 2.0 - pos.y) / TILE_SIZE).floor() as i32;
        (col, row)
    }

    /// Grid column/row range that could intersect a circle at `pos` with `radius`.
    fn cell_indices_near(&self, pos: Vec2, radius: f32) -> (i32, i32, i32, i32) {
        let (center_col, center_row) = self.world_to_cell(pos);
        let extra = (radius / TILE_SIZE).ceil() as i32 + 1;
        (
            (center_col - extra).max(0),
            (center_row - extra).max(0),
            (center_col + extra).min(self.width.saturating_sub(1) as i32),
            (center_row + extra).min(self.height.saturating_sub(1) as i32),
        )
    }

    /// Whether a circle at `center` with `radius` overlaps any wall tile.
    pub fn circle_intersects_wall(&self, center: Vec2, radius: f32) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let half = TILE_SIZE / 2.0;
        let (min_col, min_row, max_col, max_row) = self.cell_indices_near(center, radius + half);
        for row in min_row..=max_row {
            for col in min_col..=max_col {
                if self.tile_at(col, row) == Tile::Wall {
                    let cell_center = self.cell_center(col as usize, row as usize);
                    let closest = Vec2::new(
                        center.x.clamp(cell_center.x - half, cell_center.x + half),
                        center.y.clamp(cell_center.y - half, cell_center.y + half),
                    );
                    if closest.distance_squared(center) < radius * radius {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Picks the wall sprite for cell `(col, row)` from its four orthogonal
    /// neighbours. The art set has no inner (concave) corner pieces, so 4-way
    /// connectivity is exactly the right resolution.
    #[cfg(feature = "client")]
    fn wall_sprite(&self, col: i32, row: i32) -> &'static str {
        let n = self.is_wall_at(col, row - 1); // file row-1 is up / +y / North
        let e = self.is_wall_at(col + 1, row);
        let s = self.is_wall_at(col, row + 1);
        let w = self.is_wall_at(col - 1, row);

        // (north, east, south, west) -> sprite. Names describe where the *piece*
        // sits, e.g. `corner_tl` is the top-left corner of a solid block, so it
        // has neighbours to the south and east.
        match (n, e, s, w) {
            (false, false, false, false) => "walls/wall_block.png",

            // End caps: a single connection, named for which end of a bar it is.
            (true, false, false, false) => "walls/wall_bar_v_bottom.png",
            (false, false, true, false) => "walls/wall_bar_v_top.png",
            (false, true, false, false) => "walls/wall_bar_h_left.png",
            (false, false, false, true) => "walls/wall_bar_h_right.png",

            // Straight runs.
            (true, false, true, false) => "walls/wall_bar_v.png",
            (false, true, false, true) => "walls/wall_bar_h.png",

            // Outer corners (two adjacent connections).
            (true, true, false, false) => "walls/wall_corner_bl.png",
            (true, false, false, true) => "walls/wall_corner_br.png",
            (false, true, true, false) => "walls/wall_corner_tl.png",
            (false, false, true, true) => "walls/wall_corner_tr.png",

            // Edges of a thick wall mass (three connections / one open side).
            (true, true, true, false) => "walls/wall_edge_left.png",
            (true, false, true, true) => "walls/wall_edge_right.png",
            (false, true, true, true) => "walls/wall_edge_top.png",
            (true, true, false, true) => "walls/wall_edge_bottom.png",

            // Fully surrounded interior.
            (true, true, true, true) => "walls/wall_fill.png",
        }
    }
}

/// World-space rectangle the play area occupies, derived from the loaded map.
/// Movement systems and the camera read this instead of hard-coded constants.
#[derive(Resource, Clone, Copy, Debug)]
pub struct ArenaBounds {
    pub min: Vec2,
    pub max: Vec2,
}

impl ArenaBounds {
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// Clamps a point so a sprite of half-extent `half` stays fully inside.
    pub fn clamp(&self, point: Vec2, half: f32) -> Vec2 {
        Vec2::new(
            point.x.clamp(self.min.x + half, self.max.x - half),
            point.y.clamp(self.min.y + half, self.max.y - half),
        )
    }
}

/// The map the current session is being played on.
#[derive(Resource)]
pub struct CurrentMap(pub TileMap);

/// Marker for spawned wall tiles, so collision can query them later.
#[cfg(feature = "client")]
#[derive(Component)]
pub struct Wall;

/// Marker for spawned decorative/object tiles (boxes, ...), so future systems
/// (collision, interaction) can query them.
#[derive(Component)]
pub struct MapObject;

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        let map = load_map();
        let bounds = map.bounds();

        // Both client and server load the same map deterministically, so only
        // these resources are shared; the map geometry never needs replicating.
        app.insert_resource(bounds).insert_resource(CurrentMap(map));

        // Floor/wall sprites are rendered only on the client.
        #[cfg(feature = "client")]
        app.add_systems(OnEnter(GameState::Playing), spawn_map);
    }
}

/// Reads the map from disk, falling back to the embedded default if unavailable.
fn load_map() -> TileMap {
    match std::fs::read_to_string(MAP_PATH) {
        Ok(text) => TileMap::parse(&text),
        Err(err) => {
            warn!("could not read map `{MAP_PATH}` ({err}); using built-in default");
            TileMap::parse(DEFAULT_MAP)
        }
    }
}

#[cfg(feature = "client")]
fn spawn_map(mut commands: Commands, asset_server: Res<AssetServer>, map: Res<CurrentMap>) {
    use super::InGame;
    let map = &map.0;
    let floor = asset_server.load("floor-tiles.png");

    for row in 0..map.height {
        for col in 0..map.width {
            let tile = map.tile_at(col as i32, row as i32);
            if tile == Tile::Void {
                continue;
            }

            let center = map.cell_center(col, row);

            if tile.draws_floor() {
                commands.spawn((
                    Sprite {
                        image: floor.clone(),
                        custom_size: Some(Vec2::splat(TILE_SIZE)),
                        ..default()
                    },
                    Transform::from_xyz(center.x, center.y, 0.0),
                    InGame,
                ));
            }

            if tile.is_wall() {
                commands.spawn((
                    Wall,
                    Sprite {
                        image: asset_server.load(map.wall_sprite(col as i32, row as i32)),
                        custom_size: Some(Vec2::splat(TILE_SIZE)),
                        ..default()
                    },
                    Transform::from_xyz(center.x, center.y, 1.0),
                    InGame,
                ));
            }

            if let Some(sprite) = tile.object_sprite() {
                commands.spawn((
                    MapObject,
                    Sprite {
                        image: asset_server.load(sprite),
                        custom_size: Some(Vec2::splat(TILE_SIZE)),
                        ..default()
                    },
                    Transform::from_xyz(center.x, center.y, 1.0),
                    InGame,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn song_directive_selects_track_without_becoming_a_row() {
        let map = TileMap::parse("song: rocky\nwsw");
        assert_eq!(map.song(), Song::Rocky);
        // The directive line must not be parsed as a grid row.
        assert_eq!(map.width, 3);
        assert_eq!(map.height, 1);
    }

    #[test]
    fn missing_song_directive_defaults_to_shooter_loop() {
        assert_eq!(TileMap::parse("wsw").song(), Song::ShooterLoop);
    }

    #[test]
    fn unknown_song_falls_back_to_default() {
        assert_eq!(TileMap::parse("song: nope\nwsw").song(), Song::ShooterLoop);
    }

    #[test]
    fn from_name_is_case_insensitive() {
        assert_eq!(Song::from_name("Sinister"), Some(Song::Sinister));
        assert_eq!(Song::from_name("  funky "), Some(Song::Funky));
        assert_eq!(Song::from_name("does-not-exist"), None);
    }

    #[test]
    fn circle_intersects_wall_detects_nearby_walls() {
        let map = TileMap::parse("wwww\nwxxw\nwxxw\nwwww");
        let inside = map.cell_center(1, 1);
        assert!(!map.circle_intersects_wall(inside, 10.0));

        let against_right_wall = inside + Vec2::new(TILE_SIZE * 1.5, 0.0);
        assert!(map.circle_intersects_wall(against_right_wall, 10.0));

        let against_bottom_wall = inside - Vec2::new(0.0, TILE_SIZE * 1.5);
        assert!(map.circle_intersects_wall(against_bottom_wall, 10.0));
    }
}
