use bevy::prelude::*;
use bevy_replicon::prelude::*;
use serde::{Deserialize, Serialize};

use super::combat::{Dead, SpawnInvulnerability, give_spawn_invulnerability};
use super::map::{ArenaBounds, CurrentMap, TILE_SIZE};
use super::net::{NetPos, is_authoritative};
use super::pathfind;
use super::player::PlayerColor;
use super::projectile::{
    Facing, FireCooldown, Projectile, ProjectileOwner, ShotMods, tick_cooldowns, try_fire,
};
use super::shield::{ShieldState, ShieldTickSet, insert_shield};
use super::state::{GameState, MatchConfig};

pub const BOT_SIZE: f32 = 32.0;
const BOT_SPEED: f32 = 180.0;
/// How far a bot will acquire a target to hunt. Caps [`BOT_FIRE_RANGE`], since a
/// bot only shoots at a target it has already acquired.
const BOT_DETECTION_RANGE: f32 = 1100.0;
/// How close an acquired target must be before the bot opens fire. Kept just
/// under the detection range so bots start shooting as soon as they spot someone,
/// matching the now full-map shot range.
const BOT_FIRE_RANGE: f32 = 1000.0;
const BOT_AIM_THRESHOLD: f32 = 0.95;
/// Distance at which a bot considers an incoming shot dangerous enough to raise
/// its shield.
const BOT_SHIELD_RANGE: f32 = 120.0;

/// Marker for an bot. Replicated so clients know which entities to draw as
/// bots; the AI state and intent stay server-side.
#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct Bot;

/// Server-only AI state: which combatant (player or other bot) the bot is
/// currently hunting and which direction it wanders when no target is visible.
#[derive(Component, Debug, Clone, Copy)]
pub struct BotAI {
    target: Option<Entity>,
    wander: Vec2,
}

/// Server-only desired movement direction, analogous to [`PlayerIntent`].
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct BotIntent(pub Vec2);

/// Server-only cached A* path and recalculation timer.
#[derive(Component, Debug)]
struct BotPath {
    waypoints: Vec<Vec2>,
    next: usize,
    timer: Timer,
    last_target: Option<Entity>,
}

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
                bot_shield.after(ShieldTickSet),
                update_bot_paths,
                update_bot_intent,
                apply_bot_intent.after(ShieldTickSet),
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

        let entity = commands
            .spawn((
                Bot,
                BotAI {
                    target: None,
                    wander: Vec2::new(0.6, 0.8).normalize(),
                },
                BotIntent::default(),
                BotPath {
                    waypoints: Vec::new(),
                    next: 0,
                    timer: Timer::from_seconds(0.25, TimerMode::Repeating),
                    last_target: None,
                },
                PlayerColor::Red,
                NetPos(pos),
                Replicated,
                super::InGame,
            ))
            .id();
        insert_shield(&mut commands, entity);
        give_spawn_invulnerability(&mut commands, entity);
    }
}

/// Each live bot picks the nearest live combatant — any player *or other bot* —
/// as its target. Including other bots (not just players) means the surviving
/// bots turn on each other once every player is dead, so the round still
/// resolves to a single winner instead of stalling forever (see
/// `match_flow::check_for_winner`).
#[allow(clippy::type_complexity)]
fn select_bot_targets(
    mut bots: Query<(Entity, &NetPos, &mut BotAI), (With<Bot>, Without<Dead>)>,
    targets: Query<
        (Entity, &NetPos),
        (Or<(With<super::player::Player>, With<Bot>)>, Without<Dead>),
    >,
) {
    for (bot_entity, bot_pos, mut ai) in &mut bots {
        let mut nearest = None;
        let mut nearest_dist = BOT_DETECTION_RANGE * BOT_DETECTION_RANGE;

        for (target_entity, target_pos) in &targets {
            // The target set now includes bots, so skip ourselves.
            if target_entity == bot_entity {
                continue;
            }
            let dist_sq = bot_pos.0.distance_squared(target_pos.0);
            if dist_sq < nearest_dist {
                nearest_dist = dist_sq;
                nearest = Some(target_entity);
            }
        }

        ai.target = nearest;
    }
}

/// Recomputes each bot's A* path to its target on a fixed cadence.
#[allow(clippy::type_complexity)]
fn update_bot_paths(
    time: Res<Time>,
    map: Res<CurrentMap>,
    mut bots: Query<(Entity, &NetPos, &mut BotAI, &mut BotPath), (With<Bot>, Without<Dead>)>,
    targets: Query<&NetPos, (Or<(With<super::player::Player>, With<Bot>)>, Without<Dead>)>,
) {
    let radius = BOT_SIZE / 2.0;

    for (_entity, pos, ai, mut path) in &mut bots {
        path.timer.tick(time.delta());

        let Some(target_entity) = ai.target else {
            if !path.waypoints.is_empty() {
                path.waypoints.clear();
                path.next = 0;
                path.last_target = None;
                path.timer.reset();
            }
            continue;
        };

        let target_changed = path.last_target != Some(target_entity);
        if target_changed {
            path.last_target = Some(target_entity);
        }

        if !target_changed && !path.timer.just_finished() {
            continue;
        }

        if let Ok(target_pos) = targets.get(target_entity) {
            if let Some(waypoints) = pathfind::find_path(&map.0, pos.0, target_pos.0, radius) {
                path.waypoints = waypoints;
                path.next = 0;
            } else if target_changed {
                path.waypoints.clear();
                path.next = 0;
            }
        } else {
            path.waypoints.clear();
            path.next = 0;
            path.last_target = None;
        }
    }
}

/// Sets the bot's movement intent. When hunting, follow the cached A* path if
/// one exists; otherwise move straight toward the target. When no target is
/// visible, bounce around the arena like a patrol.
#[allow(clippy::type_complexity)]
fn update_bot_intent(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    mut bots: Query<
        (&NetPos, &mut BotAI, &mut BotIntent, &mut BotPath),
        (With<Bot>, Without<Dead>),
    >,
    targets: Query<&NetPos, (Or<(With<super::player::Player>, With<Bot>)>, Without<Dead>)>,
) {
    let half = BOT_SIZE / 2.0;
    let min_x = bounds.min.x + half;
    let max_x = bounds.max.x - half;
    let min_y = bounds.min.y + half;
    let max_y = bounds.max.y - half;

    let waypoint_threshold = TILE_SIZE * 0.4;
    let threshold_sq = waypoint_threshold * waypoint_threshold;

    for (pos, mut ai, mut intent, mut path) in &mut bots {
        if let Some(target) = ai.target {
            if let Ok(target_pos) = targets.get(target) {
                let mut desired = target_pos.0 - pos.0;

                // Follow cached waypoints, skipping any we have already passed.
                while path.next < path.waypoints.len() {
                    let to_waypoint = path.waypoints[path.next] - pos.0;
                    if to_waypoint.length_squared() < threshold_sq {
                        path.next += 1;
                    } else {
                        desired = to_waypoint;
                        break;
                    }
                }

                intent.0 = desired.normalize_or_zero();
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
/// Shielding bots are rooted.
#[allow(clippy::type_complexity)]
pub(crate) fn apply_bot_intent(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    map: Res<CurrentMap>,
    mut query: Query<
        (&mut NetPos, &BotIntent),
        (With<Bot>, Without<Dead>, Without<super::shield::Shielding>),
    >,
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

/// Raises a bot's shield when a hostile projectile is close. This naturally
/// blocks shots; occasional perfect parries happen when activation lines up with
/// impact timing.
#[allow(clippy::type_complexity)]
fn bot_shield(
    mut bots: Query<(Entity, &NetPos, &mut ShieldState), (With<Bot>, Without<Dead>)>,
    projectiles: Query<(&NetPos, &ProjectileOwner), With<Projectile>>,
) {
    let range_sq = BOT_SHIELD_RANGE * BOT_SHIELD_RANGE;
    for (entity, pos, mut shield) in &mut bots {
        let threatened = projectiles
            .iter()
            .any(|(p_pos, owner)| owner.0 != entity && p_pos.0.distance_squared(pos.0) < range_sq);
        shield.requested = threatened;
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
        (
            With<Bot>,
            Without<Dead>,
            Without<super::shield::Shielding>,
            Without<SpawnInvulnerability>,
        ),
    >,
    targets: Query<&NetPos, (Or<(With<super::player::Player>, With<Bot>)>, Without<Dead>)>,
) {
    for (entity, pos, facing, mut cooldown, color, ai) in bots {
        let Some(target) = ai.target else {
            continue;
        };
        let Ok(target_pos) = targets.get(target) else {
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

        // Bots don't collect power-ups in v1, so they always fire a single shot.
        try_fire(
            &mut commands,
            entity,
            *color,
            pos,
            facing,
            &mut cooldown,
            ShotMods::single(),
        );
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
        // No `InGame`: bots are replicated online (replicon owns their lifecycle);
        // offline bots carry `InGame` from `spawn_bots`. Tagging the client-side
        // replicated entity would fight replicon on the map-switch cleanup.
        commands.entity(entity).insert((
            Sprite {
                image: asset_server.load("sphere_gray.png"),
                custom_size: Some(Vec2::splat(BOT_SIZE)),
                ..default()
            },
            Transform::from_xyz(pos.0.x, pos.0.y, 10.0),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::map::TileMap;
    use crate::game::player::Player;

    /// Regression: with no players left alive, surviving bots must hunt *each
    /// other* (not give up with `target = None`), otherwise multiple bots stay
    /// alive forever and `match_flow::check_for_winner` never fires.
    #[test]
    fn bots_target_each_other_when_no_players_remain() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, select_bot_targets);

        let a = app
            .world_mut()
            .spawn((
                Bot,
                BotAI {
                    target: None,
                    wander: Vec2::X,
                },
                NetPos(Vec2::ZERO),
            ))
            .id();
        let b = app
            .world_mut()
            .spawn((
                Bot,
                BotAI {
                    target: None,
                    wander: Vec2::X,
                },
                NetPos(Vec2::new(60.0, 0.0)),
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<BotAI>(a).unwrap().target, Some(b));
        assert_eq!(app.world().get::<BotAI>(b).unwrap().target, Some(a));
    }

    /// With a wall between the bot and its target, the bot should pathfind
    /// through the gap rather than walking straight into the wall.
    #[test]
    fn bot_routes_around_wall_to_target() {
        let map = TileMap::parse(
            "wwwww\n\
             wxxww\n\
             wwxww\n\
             wxxww\n\
             wwwww",
        );

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(CurrentMap(map.clone()));
        app.insert_resource(map.bounds());
        app.add_systems(
            Update,
            (select_bot_targets, update_bot_paths, update_bot_intent).chain(),
        );

        let start = map.cell_center(1, 1);
        let goal = map.cell_center(1, 3);

        let bot = app
            .world_mut()
            .spawn((
                Bot,
                BotAI {
                    target: None,
                    wander: Vec2::X,
                },
                BotPath {
                    waypoints: Vec::new(),
                    next: 0,
                    timer: Timer::from_seconds(0.25, TimerMode::Repeating),
                    last_target: None,
                },
                BotIntent::default(),
                NetPos(start),
            ))
            .id();
        app.world_mut().spawn((Player, NetPos(goal)));

        app.update();

        let intent = app.world().get::<BotIntent>(bot).unwrap();
        assert!(
            intent.0.x > 0.0,
            "bot should detour right around the wall, intent = {:?}",
            intent.0
        );
    }
}
