//! Combat: shots damage players and enemies, who die and respawn.
//!
//! Both human players and AI enemies take damage from projectiles they do not
//! own. Health and respawn timing are server/sim-only; a small replicated
//! [`Dead`] marker lets clients hide a player or enemy during their respawn
//! delay. Everything here runs on the authoritative side (server + offline).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::enemy::Enemy;
use super::map::CurrentMap;
use super::net::{NetPos, is_authoritative};
use super::player::{PLAYER_SIZE, Player};
use super::projectile::{ImpactKind, PROJECTILE_RADIUS, Projectile, ProjectileOwner, spawn_impact};
use super::state::GameState;

/// Starting (and maximum) player health.
const MAX_HEALTH: f32 = 100.0;
/// Damage one shot deals on contact.
const PROJECTILE_DAMAGE: f32 = 25.0;
/// Seconds a player stays dead before respawning.
const RESPAWN_DELAY: f32 = 2.0;
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

/// Replicated marker present while a player or enemy is dead and awaiting
/// respawn, so clients can hide them.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Dead;

/// Server/sim-only countdown until a dead player or enemy respawns.
#[derive(Component)]
struct RespawnTimer(Timer);

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        // Authoritative: give players health, resolve hits, handle death/respawn.
        // Chained so damage → death → respawn settle within a single frame.
        app.add_systems(
            Update,
            (
                ensure_health,
                apply_projectile_hits,
                handle_deaths,
                tick_respawns,
            )
                .chain()
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );

        // Client: hide players and enemies that are currently dead.
        #[cfg(feature = "client")]
        app.add_systems(
            Update,
            hide_dead_entities.run_if(in_state(GameState::Playing)),
        );
    }
}

/// Gives any player or enemy without health a full bar (covers offline + server
/// spawns).
#[allow(clippy::type_complexity)]
fn ensure_health(
    mut commands: Commands,
    entities: Query<Entity, (Or<(With<Player>, With<Enemy>)>, Without<Health>)>,
) {
    for entity in &entities {
        commands.entity(entity).insert(Health::full());
    }
}

/// Damages the first live, non-owner player or enemy a shot touches, then
/// despawns it. Players are checked before enemies so a shot never "passes
/// through" a player to hit a bot behind them.
#[allow(clippy::type_complexity)]
fn apply_projectile_hits(
    mut commands: Commands,
    projectiles: Query<(Entity, &NetPos, &ProjectileOwner), With<Projectile>>,
    mut targets: ParamSet<(
        Query<(Entity, &NetPos, &mut Health), (With<Player>, Without<Dead>)>,
        Query<(Entity, &NetPos, &mut Health), (With<Enemy>, Without<Dead>)>,
    )>,
) {
    for (projectile, projectile_pos, owner) in &projectiles {
        let mut hit = false;

        for (player, player_pos, mut health) in targets.p0() {
            if player == owner.0 {
                continue;
            }
            if projectile_pos.0.distance(player_pos.0) <= HIT_RADIUS {
                health.current -= PROJECTILE_DAMAGE;
                spawn_impact(&mut commands, ImpactKind::Object, player_pos.0);
                hit = true;
                break;
            }
        }

        if hit {
            commands.entity(projectile).try_despawn();
            continue;
        }

        for (enemy, enemy_pos, mut health) in targets.p1() {
            if enemy == owner.0 {
                continue;
            }
            if projectile_pos.0.distance(enemy_pos.0) <= HIT_RADIUS {
                health.current -= PROJECTILE_DAMAGE;
                spawn_impact(&mut commands, ImpactKind::Object, enemy_pos.0);
                hit = true;
                break;
            }
        }

        if hit {
            commands.entity(projectile).try_despawn();
        }
    }
}

/// Marks players or enemies whose health has run out as dead and starts their
/// respawn timer.
#[allow(clippy::type_complexity)]
fn handle_deaths(
    mut commands: Commands,
    entities: Query<(Entity, &Health), (Or<(With<Player>, With<Enemy>)>, Without<Dead>)>,
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

/// Respawns dead players or enemies once their timer elapses: relocate to a
/// spawn point, refill health, and clear the dead state.
#[allow(clippy::type_complexity)]
fn tick_respawns(
    time: Res<Time>,
    map: Res<CurrentMap>,
    mut commands: Commands,
    mut entities: Query<
        (Entity, &mut NetPos, &mut Health, &mut RespawnTimer),
        Or<(With<Player>, With<Enemy>)>,
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
        }
    }
}

/// Hides dead players and enemies (and shows live ones) on the client.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn hide_dead_entities(
    mut entities: Query<(&mut Visibility, Has<Dead>), Or<(With<Player>, With<Enemy>)>>,
) {
    for (mut visibility, dead) in &mut entities {
        *visibility = if dead {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
}
