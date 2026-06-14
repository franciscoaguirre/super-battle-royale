use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::combat::Dead;
use super::map::{ArenaBounds, CurrentMap};
use super::net::{NetPos, is_authoritative, is_offline};
use super::state::GameState;

pub const PLAYER_SIZE: f32 = 32.0;
const PLAYER_SPEED: f32 = 240.0;

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
            // Movement is applied wherever the simulation is authoritative.
            .add_systems(
                Update,
                apply_player_intent
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_authoritative),
            );

        // Local input and rendering only exist in the windowed client.
        #[cfg(feature = "client")]
        app.add_systems(
            Update,
            (
                read_local_input
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_offline)
                    .before(apply_player_intent),
                attach_player_sprite.run_if(in_state(GameState::Playing)),
            ),
        );
    }
}

/// Spawns the local player for offline single-player. The sprite is attached by
/// [`attach_player_sprite`], so this only sets up the logical entity.
fn spawn_player(mut commands: Commands, selected: Res<SelectedColor>, map: Res<CurrentMap>) {
    let spawn = map.0.spawn_points().first().copied().unwrap_or(Vec2::ZERO);
    commands.spawn((
        Player,
        selected.0,
        NetPos(spawn),
        PlayerIntent::default(),
        super::InGame,
    ));
}

/// Advances every player's authoritative position from its intent, sliding
/// along map walls and clamped to the arena. Runs on the server and in offline
/// single-player.
fn apply_player_intent(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    map: Res<CurrentMap>,
    mut query: Query<(&mut NetPos, &PlayerIntent), Without<Dead>>,
) {
    let half = PLAYER_SIZE / 2.0;
    for (mut pos, intent) in &mut query {
        // Clamp the magnitude so a client can't request a higher-than-allowed speed.
        let dir = intent.0.clamp_length_max(1.0);
        let desired = pos.0 + dir * PLAYER_SPEED * time.delta_secs();

        // Slide along walls by resolving movement one axis at a time.
        let mut next = pos.0;
        let candidate_x = Vec2::new(desired.x, next.y);
        if !map.0.circle_intersects_wall(candidate_x, half) {
            next.x = candidate_x.x;
        }
        let candidate_y = Vec2::new(next.x, desired.y);
        if !map.0.circle_intersects_wall(candidate_y, half) {
            next.y = candidate_y.y;
        }

        // `set_if_neq` avoids marking the component changed (and re-replicating)
        // when a player is standing still.
        pos.set_if_neq(NetPos(bounds.clamp(next, half)));
    }
}

/// Builds a normalized movement vector from the WASD / arrow keys.
#[cfg(feature = "client")]
pub(crate) fn input_direction(input: &ButtonInput<KeyCode>) -> Vec2 {
    let mut direction = Vec2::ZERO;
    if input.pressed(KeyCode::KeyW) || input.pressed(KeyCode::ArrowUp) {
        direction.y += 1.0;
    }
    if input.pressed(KeyCode::KeyS) || input.pressed(KeyCode::ArrowDown) {
        direction.y -= 1.0;
    }
    if input.pressed(KeyCode::KeyA) || input.pressed(KeyCode::ArrowLeft) {
        direction.x -= 1.0;
    }
    if input.pressed(KeyCode::KeyD) || input.pressed(KeyCode::ArrowRight) {
        direction.x += 1.0;
    }
    if direction != Vec2::ZERO {
        direction.normalize()
    } else {
        direction
    }
}

/// Offline: feed local keyboard input into the (single) player's intent.
#[cfg(feature = "client")]
fn read_local_input(
    input: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut PlayerIntent, With<Player>>,
) {
    let dir = input_direction(&input);
    for mut intent in &mut query {
        intent.0 = dir;
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
        commands.entity(entity).insert((
            Sprite {
                image: asset_server.load(color.asset_path()),
                custom_size: Some(Vec2::splat(PLAYER_SIZE)),
                ..default()
            },
            Transform::from_xyz(pos.0.x, pos.0.y, 10.0),
            super::InGame,
        ));
    }
}
