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

use super::InGame;
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
wxxxxxxxxxxxxxxw
wxxwwxxxxxxwwxxw
wxxwwxxxxxxwwxxw
wxxxxxxsxsxxxxxw
wxxxxxxxxxxxxxxw
wxxwwxxxxxxwwxxw
wxxwwxxxxxxwwxxw
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
}

impl Tile {
    /// Maps a map-file character to a tile. Case-insensitive; unknown characters
    /// (including spaces and `.`) are treated as [`Tile::Void`].
    fn from_char(c: char) -> Tile {
        match c.to_ascii_lowercase() {
            'x' => Tile::Floor,
            'w' => Tile::Wall,
            's' => Tile::Spawn,
            _ => Tile::Void,
        }
    }

    /// Whether a floor sprite should be drawn beneath this tile. Walls are drawn
    /// on top of floor so that the transparent margins of bar/corner art blend in.
    fn draws_floor(self) -> bool {
        !matches!(self, Tile::Void)
    }

    fn is_wall(self) -> bool {
        matches!(self, Tile::Wall)
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
}

impl TileMap {
    /// Parses a map from text. Each non-empty, non-comment line is one row; rows
    /// are padded on the right with [`Tile::Void`] to the width of the widest row.
    pub fn parse(text: &str) -> TileMap {
        let rows: Vec<&str> = text
            .lines()
            .filter(|line| !line.is_empty() && !line.trim_start().starts_with('#'))
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

    fn is_wall_at(&self, col: i32, row: i32) -> bool {
        self.tile_at(col, row).is_wall()
    }

    /// Picks the wall sprite for cell `(col, row)` from its four orthogonal
    /// neighbours. The art set has no inner (concave) corner pieces, so 4-way
    /// connectivity is exactly the right resolution.
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
#[derive(Component)]
pub struct Wall;

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        let map = load_map();
        let bounds = map.bounds();

        app.insert_resource(bounds)
            .insert_resource(CurrentMap(map))
            .add_systems(OnEnter(GameState::Playing), spawn_map);
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

fn spawn_map(mut commands: Commands, asset_server: Res<AssetServer>, map: Res<CurrentMap>) {
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
        }
    }
}
