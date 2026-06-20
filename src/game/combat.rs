//! Combat: shots damage players and bots, who die and respawn.
//!
//! Both human players and AI bots take damage from projectiles they do not
//! own. Health is server/sim-only; a small replicated [`Dead`] marker lets
//! clients hide a player or bot during their respawn delay. Everything here
//! runs on the authoritative side (server + offline).

use bevy::ecs::component::Mutable;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::bot::Bot;
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
/// Multiplier applied to a shot's damage while its owner holds [`DamageBoost`].
const DAMAGE_FACTOR: f32 = 2.0;
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

/// Replicated marker present while a player or bot is dead. Death is permanent
/// for the round — a `Dead` combatant stays down (hidden, can't move or shoot)
/// until the next level, when [`reset_combatants`] revives the persistent ones.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Dead;

/// Replicated marker: the player or bot is invulnerable after spawning or
/// respawning. Removed once `remaining` reaches zero.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug)]
pub struct SpawnInvulnerability {
    pub remaining: f32,
    pub max: f32,
}

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
    damage: f32,
}

#[derive(Debug, Clone, Copy)]
enum HitKind {
    Damage,
    Parry,
    Block,
}

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
            (
                ensure_health,
                tick_spawn_invulnerability,
                apply_projectile_hits,
                apply_damage_and_blocks,
                apply_parry_reflections,
                apply_pending_heals,
                handle_deaths,
            )
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

        // Client: hide dead actors and apply spawn-invulnerability tint/opacity.
        #[cfg(feature = "client")]
        app.add_systems(
            Update,
            (hide_dead_entities, update_invulnerability_actor_visuals)
                .run_if(in_state(GameState::Playing)),
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
    commands.entity(entity).insert(SpawnInvulnerability {
        remaining: SPAWN_INVULNERABILITY_DURATION,
        max: SPAWN_INVULNERABILITY_DURATION,
    });
}

/// Ticks down spawn invulnerability and removes the marker once it expires.
fn tick_spawn_invulnerability(
    time: Res<Time>,
    mut commands: Commands,
    mut entities: Query<(Entity, &mut SpawnInvulnerability)>,
) {
    for (entity, mut inv) in &mut entities {
        inv.remaining -= time.delta_secs();
        if inv.remaining <= 0.0 {
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
    boosted: Query<(), With<DamageBoost>>,
    mut targets: ParamSet<(
        Query<
            (Entity, &NetPos, Option<&ShieldState>),
            (With<Player>, Without<Dead>, Without<SpawnInvulnerability>),
        >,
        Query<
            (Entity, &NetPos, Option<&ShieldState>),
            (With<Bot>, Without<Dead>, Without<SpawnInvulnerability>),
        >,
    )>,
) {
    for (projectile, projectile_pos, owner) in &projectiles {
        let base_damage = if boosted.contains(owner.0) {
            PROJECTILE_DAMAGE * DAMAGE_FACTOR
        } else {
            PROJECTILE_DAMAGE
        };
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
                    base_damage,
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
                    resolution = Some(build_resolution(
                        &time,
                        shield,
                        bot,
                        bot_pos.0,
                        owner.0,
                        base_damage,
                    ));
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
    shield: Option<&ShieldState>,
    target: Entity,
    target_pos: Vec2,
    owner: Entity,
    damage: f32,
) -> HitResolution {
    let kind = if let Some(shield) = shield
        && matches!(shield.status, super::shield::ShieldStatus::Active { .. })
    {
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
        damage,
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
                    let kill_shot = health.current - resolution.damage <= 0.0;
                    if kill_shot {
                        commands.entity(resolution.owner).insert(HealOnKill(1.0));
                    }
                    health.current -= resolution.damage;
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

/// Marks players or bots whose health has run out as dead. Permanent for the
/// round — there is no respawn; the `Dead` marker stays until the next level.
#[allow(clippy::type_complexity)]
fn handle_deaths(
    mut commands: Commands,
    entities: Query<(Entity, &Health), (Or<(With<Player>, With<Bot>)>, Without<Dead>)>,
    mut shields: Query<&mut ShieldState>,
) {
    for (entity, health) in &entities {
        if health.current <= 0.0 {
            commands.entity(entity).remove::<super::shield::Shielding>();
            if let Ok(mut state) = shields.get_mut(entity) {
                state.status = super::shield::ShieldStatus::Ready;
                state.charge = 1.0;
                state.requested = false;
            }
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

/// Grays out invulnerable actors and fades their opacity from 50% to 100% over
/// the spawn-protection window. Non-invulnerable actors are reset to normal.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn update_invulnerability_actor_visuals(
    mut actors: Query<(&mut Sprite, Option<&SpawnInvulnerability>), Or<(With<Player>, With<Bot>)>>,
) {
    for (mut sprite, inv) in &mut actors {
        if let Some(inv) = inv {
            let t = (inv.remaining / inv.max).clamp(0.0, 1.0);
            let alpha = 1.0 - 0.5 * t;
            sprite.color = Color::srgba(0.65, 0.65, 0.65, alpha);
        } else {
            sprite.color = Color::srgba(1.0, 1.0, 1.0, 1.0);
        }
    }
}
