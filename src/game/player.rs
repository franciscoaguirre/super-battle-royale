use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "client")]
use super::bot::Bot;
#[cfg(feature = "client")]
use super::combat::Health;
use super::combat::{Dead, SpeedBoost, give_spawn_invulnerability};
use super::map::{ArenaBounds, CurrentMap, TileMap};
use super::net::{NetPos, is_authoritative, is_offline};
#[cfg(feature = "client")]
use super::shield::ShieldState;
use super::shield::{ShieldTickSet, insert_shield};
use super::state::GameState;

pub const PLAYER_SIZE: f32 = 32.0;
const PLAYER_SPEED: f32 = 240.0;
/// Movement-speed multiplier while a player holds a [`SpeedBoost`] power-up.
const SPEED_FACTOR: f32 = 1.6;

/// Fixed simulation timestep (seconds). Player movement runs in `FixedUpdate` at
/// this rate on the authoritative side, and the client predicts/replays with the
/// exact same `dt`, so client replay reproduces server steps bit-for-bit.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// Crack overlay sprites and the health threshold at which each stage appears.
/// Thresholds are tuned for 2 HP so cracks give readable damage feedback.
#[cfg(feature = "client")]
const CRACK_STAGES: [(&str, f32); 3] = [
    ("cracks_1.png", 1.5),
    ("cracks_2.png", 1.0),
    ("cracks_3.png", 0.5),
];

/// Marker that a player already has crack-overlay children spawned.
#[cfg(feature = "client")]
#[derive(Component)]
struct HasHealthCracks;

/// Identifies which damage stage a crack-overlay child represents (1-indexed).
#[cfg(feature = "client")]
#[derive(Component)]
struct HealthCrack(u8);

/// Marker for a player avatar. Replicated so clients learn about every player.
#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct Player;

/// The desired movement direction for a player this frame. Set locally offline,
/// or from the owning client's input on the server. Never replicated — only the
/// resulting [`NetPos`] is.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PlayerIntent(pub Vec2);

/// The visual color of a player. Replicated so every client draws each player
/// with the right sprite.
#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlayerColor {
    Red,
    #[default]
    Blue,
    Green,
    Orange,
    Purple,
    Yellow,
}

impl PlayerColor {
    /// All colors, in the order the server hands them out to joining players.
    pub const ALL: [PlayerColor; 6] = [
        PlayerColor::Blue,
        PlayerColor::Red,
        PlayerColor::Green,
        PlayerColor::Orange,
        PlayerColor::Purple,
        PlayerColor::Yellow,
    ];

    /// The `n`th color to assign, wrapping around once every color is in use.
    pub fn nth(n: usize) -> PlayerColor {
        PlayerColor::ALL[n % PlayerColor::ALL.len()]
    }

    /// Path to the sprite for this color, relative to the `assets/` dir.
    pub fn asset_path(self) -> &'static str {
        match self {
            PlayerColor::Red => "sphere_red.png",
            PlayerColor::Blue => "sphere_blue.png",
            PlayerColor::Green => "sphere_green.png",
            PlayerColor::Orange => "sphere_orange.png",
            PlayerColor::Purple => "sphere_purple.png",
            PlayerColor::Yellow => "sphere_yellow.png",
        }
    }
}

/// The color the local offline player spawns with. Defaults to [`PlayerColor::Blue`].
#[derive(Resource, Default)]
pub struct SelectedColor(pub PlayerColor);

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedColor>()
            // Offline spawns the single local player; online clients receive
            // players via replication, the server via [`on_client_authorized`].
            .add_systems(OnEnter(GameState::Playing), spawn_player.run_if(is_offline))
            // Movement runs on a fixed timestep wherever the simulation is
            // authoritative, so prediction replay on the client matches it exactly.
            .add_systems(
                FixedUpdate,
                apply_player_intent
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_authoritative)
                    .after(ShieldTickSet),
            );

        // Local input and rendering only exist in the windowed client.
        #[cfg(feature = "client")]
        app.add_systems(
            Update,
            (
                read_local_input
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_offline),
                read_local_shield
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_offline)
                    .before(ShieldTickSet),
                attach_player_sprite.run_if(in_state(GameState::Playing)),
                attach_health_cracks.run_if(in_state(GameState::Playing)),
                update_health_cracks.run_if(in_state(GameState::Playing)),
            ),
        );
    }
}

/// Spawns the local player for offline single-player. The sprite is attached by
/// [`attach_player_sprite`], so this only sets up the logical entity.
fn spawn_player(mut commands: Commands, selected: Res<SelectedColor>, map: Res<CurrentMap>) {
    let spawn = map.0.spawn_points().first().copied().unwrap_or(Vec2::ZERO);
    let entity = commands
        .spawn((
            Player,
            selected.0,
            NetPos(spawn),
            PlayerIntent::default(),
            super::InGame,
        ))
        .id();
    insert_shield(&mut commands, entity);
    give_spawn_invulnerability(&mut commands, entity);
}

/// Advances a player one fixed step from a movement direction: move `dir`
/// (clamped to unit length, so a client can't request more than full speed) at
/// [`PLAYER_SPEED`] for `dt`, sliding along wall tiles one axis at a time, then
/// clamp to the arena. Pure and deterministic — shared by the authoritative
/// server step ([`apply_player_intent`]) and the client's prediction replay, so
/// both compute identical positions.
pub fn step_player(pos: Vec2, dir: Vec2, dt: f32, map: &TileMap, bounds: &ArenaBounds) -> Vec2 {
    let half = PLAYER_SIZE / 2.0;
    let desired = pos + dir.clamp_length_max(1.0) * PLAYER_SPEED * dt;

    let mut next = pos;
    let candidate_x = Vec2::new(desired.x, next.y);
    if !map.circle_intersects_wall(candidate_x, half) {
        next.x = candidate_x.x;
    }
    let candidate_y = Vec2::new(next.x, desired.y);
    if !map.circle_intersects_wall(candidate_y, half) {
        next.y = candidate_y.y;
    }

    bounds.clamp(next, half)
}

/// Advances every player's authoritative position from its intent. Runs in
/// `FixedUpdate` (fixed [`FIXED_DT`]) on the server and in offline single-player,
/// so the step is deterministic and matches the client's prediction replay.
/// `pub(crate)` so the server's `dequeue_inputs` can order itself `.before` it.
#[allow(clippy::type_complexity)]
pub(crate) fn apply_player_intent(
    bounds: Res<ArenaBounds>,
    map: Res<CurrentMap>,
    mut query: Query<
        (&mut NetPos, &PlayerIntent, Option<&SpeedBoost>),
        (Without<Dead>, Without<super::shield::Shielding>),
    >,
) {
    for (mut pos, intent, boost) in &mut query {
        // A speed power-up scales the per-tick distance by stretching `dt`; the
        // dir-clamp inside `step_player` still guards against speed-hacked inputs.
        let dt = if boost.is_some() {
            FIXED_DT * SPEED_FACTOR
        } else {
            FIXED_DT
        };
        let next = step_player(pos.0, intent.0, dt, &map.0, &bounds);
        // `set_if_neq` avoids marking the component changed (and re-replicating)
        // when a player is standing still.
        pos.set_if_neq(NetPos(next));
    }
}

/// Left-stick magnitude below which the analog stick reads as neutral. Bevy
/// already applies a per-axis deadzone via `GamepadSettings`; this is a small
/// extra radial guard so a resting stick never nudges the player.
#[cfg(feature = "client")]
const STICK_DEADZONE: f32 = 0.1;

/// Builds a movement vector from the WASD / arrow keys plus the first gamepad's
/// left stick and D-pad (if one is connected). Keyboard and pad are additive, so
/// either — or both at once — drives the player. The result is clamped to unit
/// length (so a client can't request more than full speed); analog magnitude
/// below 1.0 is preserved for fine control, and `step_player` re-clamps anyway.
#[cfg(feature = "client")]
pub(crate) fn input_direction(keys: &ButtonInput<KeyCode>, pad: Option<&Gamepad>) -> Vec2 {
    let mut direction = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        direction.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        direction.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        direction.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        direction.x += 1.0;
    }
    if let Some(pad) = pad {
        // `left_stick().y` is already +up, matching world space.
        let stick = pad.left_stick();
        if stick.length() > STICK_DEADZONE {
            direction += stick;
        }
        direction += pad.dpad();
    }
    direction.clamp_length_max(1.0)
}

/// True if the player is firing this frame: keyboard Space, or the first
/// gamepad's Cross/A (`South`) or right trigger (`R2`). The single place the
/// shoot binding is defined, shared by the offline and online fire paths.
#[cfg(feature = "client")]
pub(crate) fn shoot_just_pressed(keys: &ButtonInput<KeyCode>, pad: Option<&Gamepad>) -> bool {
    keys.just_pressed(KeyCode::Space)
        || pad.is_some_and(|p| {
            p.just_pressed(GamepadButton::South) || p.just_pressed(GamepadButton::RightTrigger2)
        })
}

/// True while the player is holding shield: keyboard Shift, or the first
/// gamepad's left trigger (`L2`) or Square/X (`West`). Shared by the offline and
/// online shield paths.
#[cfg(feature = "client")]
pub(crate) fn shield_pressed(keys: &ButtonInput<KeyCode>, pad: Option<&Gamepad>) -> bool {
    keys.pressed(KeyCode::ShiftLeft)
        || keys.pressed(KeyCode::ShiftRight)
        || pad.is_some_and(|p| {
            p.pressed(GamepadButton::LeftTrigger2) || p.pressed(GamepadButton::West)
        })
}

/// Offline: feed local keyboard/gamepad input into the (single) player's intent.
#[cfg(feature = "client")]
fn read_local_input(
    input: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut query: Query<&mut PlayerIntent, With<Player>>,
) {
    let dir = input_direction(&input, gamepads.iter().next());
    for mut intent in &mut query {
        intent.0 = dir;
    }
}

/// Offline: set the local player's shield request from Shift / L2 / Square.
#[cfg(feature = "client")]
fn read_local_shield(
    input: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut query: Query<&mut ShieldState, With<Player>>,
) {
    let pressed = shield_pressed(&input, gamepads.iter().next());
    for mut shield in &mut query {
        shield.requested = pressed;
    }
}

/// Gives any player entity that doesn't have a sprite yet (a freshly spawned
/// local player, or one just received via replication) its visual.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn attach_player_sprite(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    query: Query<(Entity, &PlayerColor, &NetPos), (With<Player>, Without<Sprite>)>,
) {
    for (entity, color, pos) in &query {
        // No `InGame`: players are replicated/persistent online (replicon owns
        // their lifecycle) and re-spawned offline; tagging them would let the
        // map-switch cleanup wrongly despawn them.
        commands.entity(entity).insert((
            Sprite {
                image: asset_server.load(color.asset_path()),
                custom_size: Some(Vec2::splat(PLAYER_SIZE)),
                ..default()
            },
            Transform::from_xyz(pos.0.x, pos.0.y, 10.0),
        ));
    }
}

/// Spawns staged crack overlays as children of any player or bot that has
/// replicated health but no cracks yet. The overlays are hidden by default and
/// revealed by [`update_health_cracks`].
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn attach_health_cracks(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    actors: Query<
        Entity,
        (
            Or<(With<Player>, With<Bot>)>,
            With<Health>,
            Without<HasHealthCracks>,
        ),
    >,
) {
    for entity in &actors {
        let mut children = Vec::with_capacity(CRACK_STAGES.len());
        for (i, (path, _)) in CRACK_STAGES.iter().enumerate() {
            let stage = (i + 1) as u8;
            let child = commands
                .spawn((
                    Sprite {
                        image: asset_server.load(*path),
                        custom_size: Some(Vec2::splat(PLAYER_SIZE)),
                        ..default()
                    },
                    Transform::from_xyz(0.0, 0.0, 0.1),
                    Visibility::Hidden,
                    HealthCrack(stage),
                    // No `InGame`: these are children of a player/bot and despawn
                    // recursively with their (replicated/persistent) parent.
                ))
                .id();
            children.push(child);
        }
        commands
            .entity(entity)
            .add_children(&children)
            .insert(HasHealthCracks);
    }
}

/// Reveals crack stages as health drops. Stages are coarse, so players and
/// bots see damage buildup without reading exact HP. Dead actors hide all
/// cracks.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn update_health_cracks(
    actors: Query<(&Health, &Children, Has<Dead>), Or<(With<Player>, With<Bot>)>>,
    mut cracks: Query<(&HealthCrack, &mut Visibility)>,
) {
    for (health, children, dead) in &actors {
        for child in children {
            if let Ok((crack, mut visibility)) = cracks.get_mut(*child) {
                let threshold = CRACK_STAGES[crack.0 as usize - 1].1;
                *visibility = if !dead && health.current <= threshold {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::map::TileMap;

    /// One fixed step covers `PLAYER_SPEED * FIXED_DT` units at full input.
    const STEP: f32 = PLAYER_SPEED * FIXED_DT;

    #[test]
    fn step_player_is_deterministic() {
        let map = TileMap::parse("xxx");
        let bounds = map.bounds();
        let a = step_player(Vec2::ZERO, Vec2::new(0.3, -0.7), FIXED_DT, &map, &bounds);
        let b = step_player(Vec2::ZERO, Vec2::new(0.3, -0.7), FIXED_DT, &map, &bounds);
        assert_eq!(a, b);
    }

    #[test]
    fn step_player_moves_at_full_speed_in_the_open() {
        let map = TileMap::parse("xxx"); // wall-free
        let bounds = map.bounds();
        let next = step_player(Vec2::ZERO, Vec2::X, FIXED_DT, &map, &bounds);
        assert!(
            (next.x - STEP).abs() < 1e-3,
            "x should advance one step, got {next:?}"
        );
        assert!(next.y.abs() < 1e-3);
    }

    #[test]
    fn step_player_slides_along_a_wall() {
        // `xwx`: a wall tile spanning world x∈[-32,32]. A player just left of it
        // (clear at x=-48, where its 16-radius only touches the wall edge) moving
        // up-and-right should be blocked on x but slide on y.
        let map = TileMap::parse("xwx");
        let bounds = map.bounds();
        let start = Vec2::new(-48.0, 0.0);
        let next = step_player(start, Vec2::new(1.0, 1.0), FIXED_DT, &map, &bounds);
        assert!(
            (next.x - start.x).abs() < 1e-3,
            "x should be blocked by the wall"
        );
        assert!(next.y > 1.0, "y should slide past the wall, got {next:?}");
    }

    /// Keyboard-only path of the gamepad-aware `input_direction` (pad = None):
    /// a single key gives a unit axis, two keys give a unit diagonal.
    #[cfg(feature = "client")]
    #[test]
    fn input_direction_keyboard_only() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyW);
        let up = input_direction(&keys, None);
        assert!((up - Vec2::Y).length() < 1e-3, "W should be +Y, got {up:?}");

        keys.press(KeyCode::KeyD);
        let diag = input_direction(&keys, None);
        assert!(
            (diag.length() - 1.0).abs() < 1e-3,
            "W+D should be a unit diagonal, got {diag:?}"
        );
        assert!(diag.x > 0.0 && diag.y > 0.0);

        keys.release(KeyCode::KeyW);
        keys.release(KeyCode::KeyD);
        assert_eq!(input_direction(&keys, None), Vec2::ZERO);
    }

    #[test]
    fn step_player_clamps_to_arena_bounds() {
        let map = TileMap::parse("xxxxx"); // bounds x∈[-160,160]; usable edge 160-16=144
        let bounds = map.bounds();
        let next = step_player(Vec2::new(143.0, 0.0), Vec2::X, FIXED_DT, &map, &bounds);
        assert!(
            (next.x - 144.0).abs() < 1e-3,
            "should clamp to arena edge, got {next:?}"
        );
    }
}
