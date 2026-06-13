use bevy::prelude::*;
use bevy_ggrs::{AddRollbackCommandExtension, PlayerInputs, Rollback, Session};

use super::InGame;
use super::map::{ArenaBounds, CurrentMap};
use crate::networking::config::{SbrConfig, INPUT_DOWN, INPUT_LEFT, INPUT_RIGHT, INPUT_UP};

pub const PLAYER_SIZE: f32 = 32.0;
const PLAYER_SPEED: f32 = 240.0;

const PLAYER_COLORS: [Color; 2] = [
    Color::srgb(0.2, 0.5, 0.95),
    Color::srgb(0.95, 0.2, 0.2),
];

#[derive(Component)]
pub struct Player {
    pub handle: usize,
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(super::state::AppState::InGame), spawn_players)
            .add_systems(bevy_ggrs::GgrsSchedule, move_players);
    }
}

fn spawn_players(mut commands: Commands, session: Res<Session<SbrConfig>>, map: Res<CurrentMap>) {
    let num_players = match &*session {
        Session::SyncTest(s) => s.num_players(),
        Session::P2P(s) => s.num_players(),
        Session::Spectator(s) => s.num_players(),
    };

    let spawns = map.0.spawn_points();

    for handle in 0..num_players {
        // Place each player on the map's spawn points in order, falling back to
        // the origin if the map declares fewer spawns than there are players.
        let position = spawns.get(handle).copied().unwrap_or(Vec2::ZERO);
        let color = PLAYER_COLORS[handle % PLAYER_COLORS.len()];

        commands
            .spawn((
                Player { handle },
                Sprite {
                    color,
                    custom_size: Some(Vec2::splat(PLAYER_SIZE)),
                    ..default()
                },
                Transform::from_xyz(position.x, position.y, 10.0),
                InGame,
            ))
            .add_rollback();
    }
}

fn move_players(
    inputs: Res<PlayerInputs<SbrConfig>>,
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    mut query: Query<(&Player, &mut Transform), With<Rollback>>,
) {
    let dt = time.delta().as_secs_f32();
    let half = PLAYER_SIZE / 2.0;

    // Query iteration order is not deterministic across peers. Sort by handle
    // so every client processes the players in the same order.
    let mut players: Vec<_> = query.iter_mut().collect();
    players.sort_by_key(|(player, _)| player.handle);

    for (player, mut transform) in players {
        let input = inputs[player.handle].0.inp;
        let mut direction = Vec2::ZERO;

        if input & INPUT_UP != 0 {
            direction.y += 1.0;
        }
        if input & INPUT_DOWN != 0 {
            direction.y -= 1.0;
        }
        if input & INPUT_LEFT != 0 {
            direction.x -= 1.0;
        }
        if input & INPUT_RIGHT != 0 {
            direction.x += 1.0;
        }

        if direction != Vec2::ZERO {
            direction = direction.normalize();
        }

        transform.translation += direction.extend(0.0) * PLAYER_SPEED * dt;
        let clamped = bounds.clamp(transform.translation.truncate(), half);
        transform.translation.x = clamped.x;
        transform.translation.y = clamped.y;
    }
}
