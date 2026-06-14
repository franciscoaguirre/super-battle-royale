//! Combat: shots damage players and bots, who die and respawn.
//!
//! Both human players and AI bots take damage from projectiles they do not
//! own. Health and respawn timing are server/sim-only; a small replicated
//! [`Dead`] marker lets clients hide a player or bot during their respawn
//! delay. Everything here runs on the authoritative side (server + offline).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::bot::Bot;
use super::map::CurrentMap;
use super::net::{NetPos, is_authoritative};
use super::player::{PLAYER_SIZE, Player};
use super::projectile::{
    ImpactKind, PROJECTILE_RADIUS, Projectile, ProjectileOwner, ProjectileVelocity, spawn_impact,
};
use super::shield::{ShieldState, is_parry_window, reflect_projectile};
use super::state::GameState;

/// Starting (and maximum) player health. Kept low so every unblocked hit is
/// threatening; the shield is the primary defensive tool.
const MAX_HEALTH: f32 = 2.0;
/// Damage one shot deals on contact.
const PROJECTILE_DAMAGE: f32 = 1.0;
/// Seconds a player stays dead before respawning.
const RESPAWN_DELAY: f32 = 2.0;
/// Seconds of invulnerability after spawning or respawning.
const SPAWN_INVULNERABILITY_DURATION: f32 = 2.0;
/// A shot hits a player when their centres are within this distance.
const HIT_RADIUS: f32 = PLAYER_SIZE / 2.0 + PROJECTILE_RADIUS;

/// A player's hit points. Replicated so every client can show damage cracks.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub(crate) fn full() -> Self {
        Self {
            current: MAX_HEALTH,
            max: MAX_HEALTH,
        }
    }
}

/// Replicated marker present while a player or bot is dead and awaiting
/// respawn, so clients can hide them.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Dead;

/// Server/sim-only countdown until a dead player or bot respawns.
#[derive(Component)]
struct RespawnTimer(Timer);

/// Server/sim-only marker: the player or bot is invulnerable after spawning
/// or respawning. Removed once [`SPAWN_INVULNERABILITY_DURATION`] elapses.
#[derive(Component)]
pub struct SpawnInvulnerability(pub Timer);

/// Server/sim-only marker: this entity should heal by the stored amount once
/// hits have been resolved. Inserted on a killer in [`apply_hit_resolutions`]
/// and consumed by [`apply_pending_heals`].
#[derive(Component)]
struct HealOnKill(f32);

/// Server/sim-only marker attached to a projectile that hit something this
/// frame, storing the result so it can be applied in a follow-up system with
/// disjoint mutable queries.
#[derive(Component, Clone, Copy)]
struct HitResolution {
    target: Entity,
    owner: Entity,
    hit_pos: Vec2,
    kind: HitKind,
}

#[derive(Debug, Clone, Copy)]
enum HitKind {
    Damage,
    Parry,
    Block,
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        // Authoritative: give players health, resolve hits, handle death/respawn.
        // Chained so damage → death → respawn settle within a single frame.
        app.add_systems(
            Update,
            (
                ensure_health,
                tick_spawn_invulnerability,
                apply_projectile_hits,
                apply_damage_and_blocks,
                apply_parry_reflections,
                apply_pending_heals,
                handle_deaths,
                tick_respawns,
            )
                .chain()
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );

        // Client: hide players and bots that are currently dead.
        #[cfg(feature = "client")]
        app.add_systems(
            Update,
            hide_dead_entities.run_if(in_state(GameState::Playing)),
        );
    }
}

/// Gives any player or bot without health a full bar (covers offline + server
/// spawns).
#[allow(clippy::type_complexity)]
fn ensure_health(
    mut commands: Commands,
    entities: Query<Entity, (Or<(With<Player>, With<Bot>)>, Without<Health>)>,
) {
    for entity in &entities {
        commands.entity(entity).insert(Health::full());
    }
}

/// Gives an entity temporary spawn invulnerability. Used by player/bot spawn
/// systems so freshly-spawned actors cannot be spawn-camped.
pub(crate) fn give_spawn_invulnerability(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .insert(SpawnInvulnerability(Timer::from_seconds(
            SPAWN_INVULNERABILITY_DURATION,
            TimerMode::Once,
        )));
}

/// Ticks down spawn invulnerability timers and removes the marker once it
/// expires.
fn tick_spawn_invulnerability(
    time: Res<Time>,
    mut commands: Commands,
    mut entities: Query<(Entity, &mut SpawnInvulnerability)>,
) {
    for (entity, mut inv) in &mut entities {
        if inv.0.tick(time.delta()).just_finished() {
            commands.entity(entity).remove::<SpawnInvulnerability>();
        }
    }
}

/// Detects projectile hits in one pass and stores the result on the
/// projectile. Applying the result (damage, reflection, despawn) happens in
/// [`apply_hit_resolutions`] with disjoint mutable queries.
#[allow(clippy::type_complexity)]
fn apply_projectile_hits(
    mut commands: Commands,
    time: Res<Time>,
    projectiles: Query<(Entity, &NetPos, &ProjectileOwner), With<Projectile>>,
    mut targets: ParamSet<(
        Query<
            (Entity, &NetPos, &ShieldState),
            (With<Player>, Without<Dead>, Without<SpawnInvulnerability>),
        >,
        Query<
            (Entity, &NetPos, &ShieldState),
            (With<Bot>, Without<Dead>, Without<SpawnInvulnerability>),
        >,
    )>,
) {
    for (projectile, projectile_pos, owner) in &projectiles {
        let mut resolution = None;

        for (player, player_pos, shield) in targets.p0() {
            if player == owner.0 {
                continue;
            }
            if projectile_pos.0.distance(player_pos.0) <= HIT_RADIUS {
                resolution = Some(build_resolution(
                    &time,
                    shield,
                    player,
                    player_pos.0,
                    owner.0,
                ));
                break;
            }
        }

        if resolution.is_none() {
            for (bot, bot_pos, shield) in targets.p1() {
                if bot == owner.0 {
                    continue;
                }
                if projectile_pos.0.distance(bot_pos.0) <= HIT_RADIUS {
                    resolution = Some(build_resolution(&time, shield, bot, bot_pos.0, owner.0));
                    break;
                }
            }
        }

        if let Some(resolution) = resolution {
            commands.entity(projectile).insert(resolution);
        }
    }
}

/// Builds a [`HitResolution`] for a projectile that reached a live target.
fn build_resolution(
    time: &Time,
    shield: &ShieldState,
    target: Entity,
    target_pos: Vec2,
    owner: Entity,
) -> HitResolution {
    let kind = if matches!(shield.status, super::shield::ShieldStatus::Active { .. }) {
        if is_parry_window(shield, time) {
            HitKind::Parry
        } else {
            HitKind::Block
        }
    } else {
        HitKind::Damage
    };
    HitResolution {
        target,
        owner,
        hit_pos: target_pos,
        kind,
    }
}

/// Applies normal damage hits and shield blocks from the [`HitResolution`]
/// components queued by [`apply_projectile_hits`]. Uses a health query that is
/// explicitly disjoint from the projectile query.
#[allow(clippy::type_complexity)]
fn apply_damage_and_blocks(
    mut commands: Commands,
    mut targets: Query<&mut Health, Without<Projectile>>,
    projectiles: Query<(Entity, &HitResolution), With<Projectile>>,
) {
    for (entity, resolution) in &projectiles {
        match resolution.kind {
            HitKind::Damage => {
                if let Ok(mut health) = targets.get_mut(resolution.target) {
                    let kill_shot = health.current - PROJECTILE_DAMAGE <= 0.0;
                    if kill_shot {
                        commands.entity(resolution.owner).insert(HealOnKill(1.0));
                    }
                    health.current -= PROJECTILE_DAMAGE;
                    spawn_impact(&mut commands, ImpactKind::Object, resolution.hit_pos);
                }
                commands.entity(entity).despawn();
            }
            HitKind::Block => {
                spawn_impact(&mut commands, ImpactKind::Shield, resolution.hit_pos);
                commands.entity(entity).despawn();
            }
            HitKind::Parry => {
                // Parries are handled by [`apply_parry_reflections`].
            }
        }
    }
}

/// Reflects projectiles marked as parries by [`apply_projectile_hits`].
#[allow(clippy::type_complexity)]
fn apply_parry_reflections(
    mut commands: Commands,
    mut projectiles: Query<
        (
            Entity,
            &mut NetPos,
            &mut ProjectileOwner,
            &mut ProjectileVelocity,
            &HitResolution,
        ),
        With<Projectile>,
    >,
) {
    for (entity, mut pos, mut owner, mut velocity, resolution) in &mut projectiles {
        if let HitKind::Parry = resolution.kind {
            reflect_projectile(
                &mut pos,
                &mut velocity,
                &mut owner,
                resolution.target,
                resolution.hit_pos,
            );
            spawn_impact(&mut commands, ImpactKind::Parry, pos.0);
            commands.entity(entity).remove::<HitResolution>();
        }
    }
}

/// Consumes queued heals, capping at max health.
#[allow(clippy::type_complexity)]
fn apply_pending_heals(
    mut commands: Commands,
    mut entities: Query<(Entity, &mut Health, &HealOnKill)>,
) {
    for (entity, mut health, heal) in &mut entities {
        health.current = (health.current + heal.0).min(health.max);
        commands.entity(entity).remove::<HealOnKill>();
    }
}

/// Marks players or bots whose health has run out as dead and starts their
/// respawn timer.
#[allow(clippy::type_complexity)]
fn handle_deaths(
    mut commands: Commands,
    entities: Query<(Entity, &Health), (Or<(With<Player>, With<Bot>)>, Without<Dead>)>,
) {
    for (entity, health) in &entities {
        if health.current <= 0.0 {
            commands.entity(entity).insert((
                Dead,
                RespawnTimer(Timer::from_seconds(RESPAWN_DELAY, TimerMode::Once)),
            ));
        }
    }
}

/// Respawns dead players or bots once their timer elapses: relocate to a
/// spawn point, refill health, and clear the dead state.
#[allow(clippy::type_complexity)]
fn tick_respawns(
    time: Res<Time>,
    map: Res<CurrentMap>,
    mut commands: Commands,
    mut entities: Query<
        (Entity, &mut NetPos, &mut Health, &mut RespawnTimer),
        Or<(With<Player>, With<Bot>)>,
    >,
) {
    for (entity, mut pos, mut health, mut timer) in &mut entities {
        if timer.0.tick(time.delta()).just_finished() {
            let spawns = map.0.spawn_points();
            if !spawns.is_empty() {
                pos.0 = spawns[entity.to_bits() as usize % spawns.len()];
            }
            health.current = health.max;
            commands.entity(entity).remove::<(Dead, RespawnTimer)>();
            commands
                .entity(entity)
                .insert(SpawnInvulnerability(Timer::from_seconds(
                    SPAWN_INVULNERABILITY_DURATION,
                    TimerMode::Once,
                )));
        }
    }
}

/// Hides dead players and bots (and shows live ones) on the client.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn hide_dead_entities(
    mut entities: Query<(&mut Visibility, Has<Dead>), Or<(With<Player>, With<Bot>)>>,
) {
    for (mut visibility, dead) in &mut entities {
        *visibility = if dead {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
}
