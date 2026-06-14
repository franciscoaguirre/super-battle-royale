use bevy::prelude::*;
use bevy_replicon::prelude::*;
use serde::{Deserialize, Serialize};

use super::map::ArenaBounds;
use super::net::{NetPos, is_authoritative};
use super::state::GameState;

pub const ENEMY_SIZE: f32 = 32.0;
const ENEMY_SPEED: f32 = 120.0;

/// Marker for an enemy. Replicated so clients know which entities to draw as
/// enemies; the patrol direction stays server-side in [`EnemyPatrol`].
#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct Enemy;

/// Server-only patrol state: the current heading the enemy is bouncing along.
#[derive(Component, Debug, Clone, Copy)]
pub struct EnemyPatrol {
    direction: Vec2,
}

pub struct EnemyPlugin;

impl Plugin for EnemyPlugin {
    fn build(&self, app: &mut App) {
        // Enemies are simulated wherever we're authoritative (server or offline).
        app.add_systems(
            OnEnter(GameState::Playing),
            spawn_enemies.run_if(is_authoritative),
        )
        .add_systems(
            Update,
            patrol_enemies
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );

        #[cfg(feature = "client")]
        app.add_systems(
            Update,
            attach_enemy_sprite.run_if(in_state(GameState::Playing)),
        );
    }
}

/// Spawns the authoritative enemy entities. `Replicated` is inert offline (no
/// server running) and drives replication on the dedicated server.
fn spawn_enemies(mut commands: Commands) {
    let positions = [
        Vec2::new(-200.0, 150.0),
        Vec2::new(200.0, -150.0),
        Vec2::new(-150.0, -200.0),
    ];

    for pos in positions {
        commands.spawn((
            Enemy,
            EnemyPatrol {
                direction: Vec2::new(0.6, 0.8).normalize(),
            },
            NetPos(pos),
            Replicated,
        ));
    }
}

/// Moves enemies, bouncing them off the arena walls. Authoritative side only.
fn patrol_enemies(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    mut query: Query<(&mut NetPos, &mut EnemyPatrol)>,
) {
    let half = ENEMY_SIZE / 2.0;
    let min_x = bounds.min.x + half;
    let max_x = bounds.max.x - half;
    let min_y = bounds.min.y + half;
    let max_y = bounds.max.y - half;

    for (mut pos, mut patrol) in &mut query {
        let delta = patrol.direction * ENEMY_SPEED * time.delta_secs();
        let next = pos.0 + delta;

        // Bounce off arena walls.
        if next.x < min_x || next.x > max_x {
            patrol.direction.x *= -1.0;
        }
        if next.y < min_y || next.y > max_y {
            patrol.direction.y *= -1.0;
        }

        pos.0 += patrol.direction * ENEMY_SPEED * time.delta_secs();
    }
}

/// Gives any enemy entity without a sprite (offline-spawned or replicated in)
/// its red-square visual.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn attach_enemy_sprite(
    mut commands: Commands,
    query: Query<(Entity, &NetPos), (With<Enemy>, Without<Sprite>)>,
) {
    for (entity, pos) in &query {
        commands.entity(entity).insert((
            Sprite {
                color: Color::srgb(0.95, 0.2, 0.2),
                custom_size: Some(Vec2::splat(ENEMY_SIZE)),
                ..default()
            },
            Transform::from_xyz(pos.0.x, pos.0.y, 10.0),
            super::InGame,
        ));
    }
}
