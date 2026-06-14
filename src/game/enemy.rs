use bevy::prelude::*;
use bevy_replicon::prelude::*;
use serde::{Deserialize, Serialize};

use super::combat::Dead;
use super::map::{ArenaBounds, CurrentMap};
use super::net::{NetPos, is_authoritative};
use super::player::PlayerColor;
use super::projectile::{Facing, FireCooldown, tick_cooldowns, try_fire};
use super::state::GameState;

pub const ENEMY_SIZE: f32 = 32.0;
const ENEMY_SPEED: f32 = 180.0;
const ENEMY_DETECTION_RANGE: f32 = 500.0;
const ENEMY_FIRE_RANGE: f32 = 280.0;
const ENEMY_AIM_THRESHOLD: f32 = 0.95;

/// Marker for an enemy. Replicated so clients know which entities to draw as
/// enemies; the AI state and intent stay server-side.
#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct Enemy;

/// Server-only AI state: which player the enemy is currently hunting and which
/// direction it wanders when no target is visible.
#[derive(Component, Debug, Clone, Copy)]
pub struct EnemyAI {
    target: Option<Entity>,
    wander: Vec2,
}

/// Server-only desired movement direction, analogous to [`PlayerIntent`].
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct EnemyIntent(pub Vec2);

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
            (
                select_enemy_targets,
                update_enemy_intent,
                apply_enemy_intent,
                update_enemy_facing,
                enemy_shoot.after(tick_cooldowns),
            )
                .chain()
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
fn spawn_enemies(mut commands: Commands, map: Res<CurrentMap>) {
    let spawns = map.0.spawn_points();
    let count = 3;

    for i in 0..count {
        let pos = if spawns.is_empty() {
            Vec2::ZERO
        } else {
            // Offset enemy spawns so they don't all start on top of player 0.
            spawns[(i + 1) % spawns.len()]
        };

        commands.spawn((
            Enemy,
            EnemyAI {
                target: None,
                wander: Vec2::new(0.6, 0.8).normalize(),
            },
            EnemyIntent::default(),
            PlayerColor::Red,
            NetPos(pos),
            Replicated,
            super::InGame,
        ));
    }
}

/// Each live enemy picks the nearest live player as its target.
#[allow(clippy::type_complexity)]
fn select_enemy_targets(
    mut enemies: Query<(Entity, &NetPos, &mut EnemyAI), (With<Enemy>, Without<Dead>)>,
    players: Query<(Entity, &NetPos), (With<super::player::Player>, Without<Dead>)>,
) {
    for (enemy_entity, enemy_pos, mut ai) in &mut enemies {
        let mut nearest = None;
        let mut nearest_dist = ENEMY_DETECTION_RANGE * ENEMY_DETECTION_RANGE;

        for (player_entity, player_pos) in &players {
            // Don't target yourself (relevant if enemies ever get a Player tag).
            if player_entity == enemy_entity {
                continue;
            }
            let dist_sq = enemy_pos.0.distance_squared(player_pos.0);
            if dist_sq < nearest_dist {
                nearest_dist = dist_sq;
                nearest = Some(player_entity);
            }
        }

        ai.target = nearest;
    }
}

/// Sets the enemy's movement intent. When hunting, move straight toward the
/// target; otherwise bounce around the arena like a patrol.
#[allow(clippy::type_complexity)]
fn update_enemy_intent(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    mut enemies: Query<(&NetPos, &mut EnemyAI, &mut EnemyIntent), (With<Enemy>, Without<Dead>)>,
    players: Query<&NetPos, (With<super::player::Player>, Without<Dead>)>,
) {
    let half = ENEMY_SIZE / 2.0;
    let min_x = bounds.min.x + half;
    let max_x = bounds.max.x - half;
    let min_y = bounds.min.y + half;
    let max_y = bounds.max.y - half;

    for (pos, mut ai, mut intent) in &mut enemies {
        if let Some(target) = ai.target {
            if let Ok(target_pos) = players.get(target) {
                let to_target = target_pos.0 - pos.0;
                intent.0 = to_target.normalize_or_zero();
                continue;
            }
            ai.target = None;
        }

        // No target: wander and bounce off the outer arena walls.
        let next = pos.0 + ai.wander * ENEMY_SPEED * time.delta_secs();

        if next.x < min_x || next.x > max_x {
            ai.wander.x *= -1.0;
        }
        if next.y < min_y || next.y > max_y {
            ai.wander.y *= -1.0;
        }

        intent.0 = ai.wander;
    }
}

/// Advances every enemy's authoritative position from its intent, sliding along
/// map walls and clamped to the arena. Mirrors [`apply_player_intent`].
#[allow(clippy::type_complexity)]
fn apply_enemy_intent(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    map: Res<CurrentMap>,
    mut query: Query<(&mut NetPos, &EnemyIntent), (With<Enemy>, Without<Dead>)>,
) {
    let half = ENEMY_SIZE / 2.0;
    for (mut pos, intent) in &mut query {
        let dir = intent.0.clamp_length_max(1.0);
        let desired = pos.0 + dir * ENEMY_SPEED * time.delta_secs();

        let mut next = pos.0;
        let candidate_x = Vec2::new(desired.x, next.y);
        if !map.0.circle_intersects_wall(candidate_x, half) {
            next.x = candidate_x.x;
        }
        let candidate_y = Vec2::new(next.x, desired.y);
        if !map.0.circle_intersects_wall(candidate_y, half) {
            next.y = candidate_y.y;
        }

        pos.set_if_neq(NetPos(bounds.clamp(next, half)));
    }
}

/// Keeps the enemy's facing aligned with its movement intent so shots fly in
/// the direction it is moving.
fn update_enemy_facing(mut query: Query<(&EnemyIntent, &mut Facing), With<Enemy>>) {
    for (intent, mut facing) in &mut query {
        if intent.0 != Vec2::ZERO {
            facing.0 = intent.0.normalize_or_zero();
        }
    }
}

/// Fires a shot when a target is within range and the enemy is roughly aiming
/// at it. Respects the same fire cooldown as players.
#[allow(clippy::type_complexity)]
fn enemy_shoot(
    mut commands: Commands,
    enemies: Query<
        (
            Entity,
            &NetPos,
            &Facing,
            &mut FireCooldown,
            &PlayerColor,
            &EnemyAI,
        ),
        (With<Enemy>, Without<Dead>),
    >,
    players: Query<&NetPos, (With<super::player::Player>, Without<Dead>)>,
) {
    for (entity, pos, facing, mut cooldown, color, ai) in enemies {
        let Some(target) = ai.target else {
            continue;
        };
        let Ok(target_pos) = players.get(target) else {
            continue;
        };

        let to_target = target_pos.0 - pos.0;
        if to_target.length_squared() > ENEMY_FIRE_RANGE * ENEMY_FIRE_RANGE {
            continue;
        }

        let aim = to_target.normalize_or_zero();
        if aim.dot(facing.0) < ENEMY_AIM_THRESHOLD {
            continue;
        }

        try_fire(&mut commands, entity, *color, pos, facing, &mut cooldown);
    }
}

/// Gives any enemy entity without a sprite (offline-spawned or replicated in)
/// its red-sphere visual.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn attach_enemy_sprite(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    query: Query<(Entity, &NetPos), (With<Enemy>, Without<Sprite>)>,
) {
    for (entity, pos) in &query {
        commands.entity(entity).insert((
            Sprite {
                image: asset_server.load("sphere_gray.png"),
                custom_size: Some(Vec2::splat(ENEMY_SIZE)),
                ..default()
            },
            Transform::from_xyz(pos.0.x, pos.0.y, 10.0),
            super::InGame,
        ));
    }
}
