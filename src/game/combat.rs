//! Combat: shots damage players and bots, who die and respawn.
//!
//! Both human players and AI bots take damage from projectiles they do not
//! own. Health and respawn timing are server/sim-only; a small replicated
//! [`Dead`] marker lets clients hide a player or bot during their respawn
//! delay. Everything here runs on the authoritative side (server + offline).

use bevy::ecs::component::Mutable;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::bot::Bot;
use super::net::{NetPos, is_authoritative};
use super::player::{PLAYER_SIZE, Player};
use super::projectile::{ImpactKind, PROJECTILE_RADIUS, Projectile, ProjectileOwner, spawn_impact};
use super::state::GameState;

/// Starting (and maximum) player health.
const MAX_HEALTH: f32 = 100.0;
/// Damage one shot deals on contact.
const PROJECTILE_DAMAGE: f32 = 25.0;
/// Multiplier applied to a shot's damage while its owner holds [`DamageBoost`].
const DAMAGE_FACTOR: f32 = 2.0;
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

/// Replicated marker present while a player or bot is dead. Death is permanent
/// for the round — a `Dead` combatant stays down (hidden, can't move or shoot)
/// until the next level, when [`reset_combatants`] revives the persistent ones.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Dead;

/// A timed power-up effect. Each buff is a newtype around a [`Timer`] that lives
/// on a player while the effect is active; [`tick_buff`] ticks it and removes the
/// component when the timer runs out, so "buffed" is simply "the component is
/// present" — systems honour a buff by querying `Option<&Buff>` / `With<Buff>`.
/// Buffs are authoritative-only (their *results* — position, health, projectiles
/// — already replicate), and granted by `pickup::collect_pickups`.
pub trait BuffTimer {
    fn timer(&mut self) -> &mut Timer;
}

/// Defines a timed-buff component (a `Timer` newtype) and its [`BuffTimer`] impl.
macro_rules! buff {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Component, Debug)]
        pub struct $name(pub Timer);

        impl BuffTimer for $name {
            fn timer(&mut self) -> &mut Timer {
                &mut self.0
            }
        }
    };
}

buff!(
    /// Multiplies the holder's movement speed (see `player::apply_player_intent`).
    SpeedBoost
);
buff!(
    /// Speeds up the holder's fire rate (see `projectile::tick_cooldowns`).
    RapidFire
);
buff!(
    /// Multiplies the damage of shots the holder fires (see `apply_projectile_hits`).
    DamageBoost
);
buff!(
    /// Makes the holder fire forward *and* backward (see `projectile::try_fire`).
    DoubleShot
);
buff!(
    /// Makes the holder fire in a four-way cross (see `projectile::try_fire`).
    QuadShot
);
buff!(
    /// Makes the holder's shots weave from side to side (see `projectile::simulate_projectiles`).
    Zigzag
);

/// Ticks one kind of timed buff and strips it from any entity whose timer has
/// run out, so buffs expire on their own. Registered once per buff type.
fn tick_buff<B: Component<Mutability = Mutable> + BuffTimer>(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut B)>,
) {
    for (entity, mut buff) in &mut query {
        if buff.timer().tick(time.delta()).just_finished() {
            commands.entity(entity).remove::<B>();
        }
    }
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        // Authoritative: give players health and resolve hits → death. Chained so
        // damage and death settle within a single frame. Death is permanent for
        // the round (no respawn); combatants are revived on the next level.
        app.add_systems(
            Update,
            (ensure_health, apply_projectile_hits, handle_deaths)
                .chain()
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        )
        // Revive survivors (persistent online players) when a new round starts.
        .add_systems(
            OnEnter(GameState::Playing),
            reset_combatants.run_if(is_authoritative),
        );

        // Authoritative: expire timed power-up buffs once their timers run out.
        // Unordered — a one-frame-stale buff is imperceptible.
        app.add_systems(
            Update,
            (
                tick_buff::<SpeedBoost>,
                tick_buff::<RapidFire>,
                tick_buff::<DamageBoost>,
                tick_buff::<DoubleShot>,
                tick_buff::<QuadShot>,
                tick_buff::<Zigzag>,
            )
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

/// Damages the first live, non-owner player or bot a shot touches, then
/// despawns it. Players are checked before bots so a shot never "passes
/// through" a player to hit a bot behind them.
#[allow(clippy::type_complexity)]
fn apply_projectile_hits(
    mut commands: Commands,
    projectiles: Query<(Entity, &NetPos, &ProjectileOwner), With<Projectile>>,
    // Owners currently holding a damage power-up; read-only and disjoint from the
    // mutable `Health` queries below, so it needs no `ParamSet` slot.
    boosted: Query<(), With<DamageBoost>>,
    mut targets: ParamSet<(
        Query<(Entity, &NetPos, &mut Health), (With<Player>, Without<Dead>)>,
        Query<(Entity, &NetPos, &mut Health), (With<Bot>, Without<Dead>)>,
    )>,
) {
    for (projectile, projectile_pos, owner) in &projectiles {
        let mut hit = false;
        let damage = if boosted.contains(owner.0) {
            PROJECTILE_DAMAGE * DAMAGE_FACTOR
        } else {
            PROJECTILE_DAMAGE
        };

        for (player, player_pos, mut health) in targets.p0() {
            if player == owner.0 {
                continue;
            }
            if projectile_pos.0.distance(player_pos.0) <= HIT_RADIUS {
                health.current -= damage;
                spawn_impact(&mut commands, ImpactKind::Object, player_pos.0);
                hit = true;
                break;
            }
        }

        if hit {
            commands.entity(projectile).try_despawn();
            continue;
        }

        for (bot, bot_pos, mut health) in targets.p1() {
            if bot == owner.0 {
                continue;
            }
            if projectile_pos.0.distance(bot_pos.0) <= HIT_RADIUS {
                health.current -= damage;
                spawn_impact(&mut commands, ImpactKind::Object, bot_pos.0);
                hit = true;
                break;
            }
        }

        if hit {
            commands.entity(projectile).try_despawn();
        }
    }
}

/// Marks players or bots whose health has run out as dead. Permanent for the
/// round — there is no respawn; the `Dead` marker stays until the next level.
#[allow(clippy::type_complexity)]
fn handle_deaths(
    mut commands: Commands,
    entities: Query<(Entity, &Health), (Or<(With<Player>, With<Bot>)>, Without<Dead>)>,
) {
    for (entity, health) in &entities {
        if health.current <= 0.0 {
            commands.entity(entity).insert(Dead);
        }
    }
}

/// Revives every combatant carried over from a previous round (the persistent
/// online players) when a new round starts: full health and `Dead` cleared.
/// Fresh bots and the offline player are re-spawned each round and get full
/// health from [`ensure_health`] instead; positions are set by `position_players`
/// / `spawn_*`. Harmless on the first round (nothing has `Health` yet).
#[allow(clippy::type_complexity)]
fn reset_combatants(
    mut commands: Commands,
    mut combatants: Query<(Entity, &mut Health), Or<(With<Player>, With<Bot>)>>,
) {
    for (entity, mut health) in &mut combatants {
        health.current = health.max;
        commands.entity(entity).remove::<Dead>();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::net::NetRole;
    use bevy::state::app::StatesPlugin;

    #[test]
    fn death_is_permanent_within_a_round_and_reset_revives_next_round() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin, CombatPlugin));
        app.insert_resource(NetRole::Server); // authoritative
        app.insert_state(GameState::Playing);

        let player = app.world_mut().spawn(Player).id();
        app.update(); // ensure_health → full
        assert_eq!(app.world().get::<Health>(player).unwrap().current, 100.0);

        app.world_mut().get_mut::<Health>(player).unwrap().current = 0.0;
        app.update(); // handle_deaths → Dead
        assert!(app.world().get::<Dead>(player).is_some());

        // No respawn: stays dead across many frames.
        for _ in 0..120 {
            app.update();
        }
        assert!(
            app.world().get::<Dead>(player).is_some(),
            "death must be permanent within a round"
        );

        // A new round (re-enter Playing) revives via reset_combatants.
        app.world_mut()
            .resource_mut::<NextState<GameState>>()
            .set(GameState::GameOver);
        app.update();
        app.world_mut()
            .resource_mut::<NextState<GameState>>()
            .set(GameState::Playing);
        app.update();
        assert!(
            app.world().get::<Dead>(player).is_none(),
            "a new round should revive the player"
        );
        assert_eq!(app.world().get::<Health>(player).unwrap().current, 100.0);
    }
}
