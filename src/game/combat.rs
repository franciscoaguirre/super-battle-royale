//! PvP combat: shots damage players, who die and respawn.
//!
//! Only players take damage — the patrolling enemies ("bots") are intentionally
//! ignored, so shots pass through them. Health and respawn timing are
//! server/sim-only; a small replicated [`Dead`] marker lets clients hide a player
//! during their respawn delay. Everything here runs on the authoritative side
//! (server + offline); offline single-player simply never has another player to
//! hit, so nobody takes damage.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

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

/// A player's hit points. Server/sim-only (no HUD yet, so not replicated).
#[derive(Component, Clone, Copy, Debug)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    fn full() -> Self {
        Self {
            current: MAX_HEALTH,
            max: MAX_HEALTH,
        }
    }
}

/// Replicated marker present while a player is dead and awaiting respawn, so
/// clients can hide them.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Dead;

/// Server/sim-only countdown until a dead player respawns.
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
                ensure_player_health,
                apply_projectile_hits,
                handle_deaths,
                tick_respawns,
            )
                .chain()
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );

        // Client: hide players that are currently dead.
        #[cfg(feature = "client")]
        app.add_systems(
            Update,
            hide_dead_players.run_if(in_state(GameState::Playing)),
        );
    }
}

/// Gives any player without health a full bar (covers offline + server spawns).
fn ensure_player_health(
    mut commands: Commands,
    players: Query<Entity, (With<Player>, Without<Health>)>,
) {
    for entity in &players {
        commands.entity(entity).insert(Health::full());
    }
}

/// Damages the first live, non-owner player a shot touches, then despawns it.
#[allow(clippy::type_complexity)]
fn apply_projectile_hits(
    mut commands: Commands,
    projectiles: Query<(Entity, &NetPos, &ProjectileOwner), With<Projectile>>,
    mut players: Query<(Entity, &NetPos, &mut Health), (With<Player>, Without<Dead>)>,
) {
    for (projectile, projectile_pos, owner) in &projectiles {
        for (player, player_pos, mut health) in &mut players {
            if player == owner.0 {
                continue;
            }
            if projectile_pos.0.distance(player_pos.0) <= HIT_RADIUS {
                health.current -= PROJECTILE_DAMAGE;
                spawn_impact(&mut commands, ImpactKind::Object);
                commands.entity(projectile).try_despawn();
                break; // a shot hits at most one player
            }
        }
    }
}

/// Marks players whose health has run out as dead and starts their respawn timer.
#[allow(clippy::type_complexity)]
fn handle_deaths(
    mut commands: Commands,
    players: Query<(Entity, &Health), (With<Player>, Without<Dead>)>,
) {
    for (entity, health) in &players {
        if health.current <= 0.0 {
            commands.entity(entity).insert((
                Dead,
                RespawnTimer(Timer::from_seconds(RESPAWN_DELAY, TimerMode::Once)),
            ));
        }
    }
}

/// Respawns dead players once their timer elapses: relocate to a spawn point,
/// refill health, and clear the dead state.
fn tick_respawns(
    time: Res<Time>,
    map: Res<CurrentMap>,
    mut commands: Commands,
    mut players: Query<(Entity, &mut NetPos, &mut Health, &mut RespawnTimer), With<Player>>,
) {
    for (entity, mut pos, mut health, mut timer) in &mut players {
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

/// Hides dead players (and shows live ones) on the client.
#[cfg(feature = "client")]
fn hide_dead_players(mut players: Query<(&mut Visibility, Has<Dead>), With<Player>>) {
    for (mut visibility, dead) in &mut players {
        *visibility = if dead {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
}
