use bevy::prelude::*;
use bevy_replicon::prelude::*;
use serde::{Deserialize, Serialize};

use super::combat::Dead;
use super::map::{ArenaBounds, CurrentMap};
use super::net::{NetPos, is_authoritative};
use super::player::PlayerColor;
use super::projectile::{Facing, FireCooldown, tick_cooldowns, try_fire};
use super::state::{GameState, MatchConfig};

pub const BOT_SIZE: f32 = 32.0;
const BOT_SPEED: f32 = 180.0;
const BOT_DETECTION_RANGE: f32 = 500.0;
const BOT_FIRE_RANGE: f32 = 280.0;
const BOT_AIM_THRESHOLD: f32 = 0.95;

/// Marker for an bot. Replicated so clients know which entities to draw as
/// bots; the AI state and intent stay server-side.
#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct Bot;

/// Server-only AI state: which player the bot is currently hunting and which
/// direction it wanders when no target is visible.
#[derive(Component, Debug, Clone, Copy)]
pub struct BotAI {
    target: Option<Entity>,
    wander: Vec2,
}

/// Server-only desired movement direction, analogous to [`PlayerIntent`].
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct BotIntent(pub Vec2);

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        // Enemies are simulated wherever we're authoritative (server or offline).
        app.add_systems(
            OnEnter(GameState::Playing),
            spawn_bots.run_if(is_authoritative),
        )
        .add_systems(
            Update,
            (
                select_bot_targets,
                update_bot_intent,
                apply_bot_intent,
                update_bot_facing,
                bot_shoot.after(tick_cooldowns),
            )
                .chain()
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );

        #[cfg(feature = "client")]
        app.add_systems(
            Update,
            attach_bot_sprite.run_if(in_state(GameState::Playing)),
        );
    }
}

/// Spawns the authoritative bot entities. `Replicated` is inert offline (no
/// server running) and drives replication on the dedicated server.
fn spawn_bots(mut commands: Commands, map: Res<CurrentMap>, config: Res<MatchConfig>) {
    let spawns = map.0.spawn_points();
    let count = config.bot_count as usize;

    for i in 0..count {
        let pos = if spawns.is_empty() {
            Vec2::ZERO
        } else {
            // Offset bot spawns so they don't all start on top of player 0.
            spawns[(i + 1) % spawns.len()]
        };

        commands.spawn((
            Bot,
            BotAI {
                target: None,
                wander: Vec2::new(0.6, 0.8).normalize(),
            },
            BotIntent::default(),
            PlayerColor::Red,
            NetPos(pos),
            Replicated,
            super::InGame,
        ));
    }
}

/// Each live bot picks the nearest live player as its target.
#[allow(clippy::type_complexity)]
fn select_bot_targets(
    mut bots: Query<(Entity, &NetPos, &mut BotAI), (With<Bot>, Without<Dead>)>,
    players: Query<(Entity, &NetPos), (With<super::player::Player>, Without<Dead>)>,
) {
    for (bot_entity, bot_pos, mut ai) in &mut bots {
        let mut nearest = None;
        let mut nearest_dist = BOT_DETECTION_RANGE * BOT_DETECTION_RANGE;

        for (player_entity, player_pos) in &players {
            // Don't target yourself (relevant if bots ever get a Player tag).
            if player_entity == bot_entity {
                continue;
            }
            let dist_sq = bot_pos.0.distance_squared(player_pos.0);
            if dist_sq < nearest_dist {
                nearest_dist = dist_sq;
                nearest = Some(player_entity);
            }
        }

        ai.target = nearest;
    }
}

/// Sets the bot's movement intent. When hunting, move straight toward the
/// target; otherwise bounce around the arena like a patrol.
#[allow(clippy::type_complexity)]
fn update_bot_intent(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    mut bots: Query<(&NetPos, &mut BotAI, &mut BotIntent), (With<Bot>, Without<Dead>)>,
    players: Query<&NetPos, (With<super::player::Player>, Without<Dead>)>,
) {
    let half = BOT_SIZE / 2.0;
    let min_x = bounds.min.x + half;
    let max_x = bounds.max.x - half;
    let min_y = bounds.min.y + half;
    let max_y = bounds.max.y - half;

    for (pos, mut ai, mut intent) in &mut bots {
        if let Some(target) = ai.target {
            if let Ok(target_pos) = players.get(target) {
                let to_target = target_pos.0 - pos.0;
                intent.0 = to_target.normalize_or_zero();
                continue;
            }
            ai.target = None;
        }

        // No target: wander and bounce off the outer arena walls.
        let next = pos.0 + ai.wander * BOT_SPEED * time.delta_secs();

        if next.x < min_x || next.x > max_x {
            ai.wander.x *= -1.0;
        }
        if next.y < min_y || next.y > max_y {
            ai.wander.y *= -1.0;
        }

        intent.0 = ai.wander;
    }
}

/// Advances every bot's authoritative position from its intent, sliding along
/// map walls and clamped to the arena. Mirrors [`apply_player_intent`].
#[allow(clippy::type_complexity)]
fn apply_bot_intent(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    map: Res<CurrentMap>,
    mut query: Query<(&mut NetPos, &BotIntent), (With<Bot>, Without<Dead>)>,
) {
    let half = BOT_SIZE / 2.0;
    for (mut pos, intent) in &mut query {
        let dir = intent.0.clamp_length_max(1.0);
        let desired = pos.0 + dir * BOT_SPEED * time.delta_secs();

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

/// Keeps the bot's facing aligned with its movement intent so shots fly in
/// the direction it is moving.
fn update_bot_facing(mut query: Query<(&BotIntent, &mut Facing), With<Bot>>) {
    for (intent, mut facing) in &mut query {
        if intent.0 != Vec2::ZERO {
            facing.0 = intent.0.normalize_or_zero();
        }
    }
}

/// Fires a shot when a target is within range and the bot is roughly aiming
/// at it. Respects the same fire cooldown as players.
#[allow(clippy::type_complexity)]
fn bot_shoot(
    mut commands: Commands,
    bots: Query<
        (
            Entity,
            &NetPos,
            &Facing,
            &mut FireCooldown,
            &PlayerColor,
            &BotAI,
        ),
        (With<Bot>, Without<Dead>),
    >,
    players: Query<&NetPos, (With<super::player::Player>, Without<Dead>)>,
) {
    for (entity, pos, facing, mut cooldown, color, ai) in bots {
        let Some(target) = ai.target else {
            continue;
        };
        let Ok(target_pos) = players.get(target) else {
            continue;
        };

        let to_target = target_pos.0 - pos.0;
        if to_target.length_squared() > BOT_FIRE_RANGE * BOT_FIRE_RANGE {
            continue;
        }

        let aim = to_target.normalize_or_zero();
        if aim.dot(facing.0) < BOT_AIM_THRESHOLD {
            continue;
        }

        try_fire(&mut commands, entity, *color, pos, facing, &mut cooldown);
    }
}

/// Gives any bot entity without a sprite (offline-spawned or replicated in)
/// its red-sphere visual.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn attach_bot_sprite(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    query: Query<(Entity, &NetPos), (With<Bot>, Without<Sprite>)>,
) {
    for (entity, pos) in &query {
        commands.entity(entity).insert((
            Sprite {
                image: asset_server.load("sphere_gray.png"),
                custom_size: Some(Vec2::splat(BOT_SIZE)),
                ..default()
            },
            Transform::from_xyz(pos.0.x, pos.0.y, 10.0),
            super::InGame,
        ));
    }
}
