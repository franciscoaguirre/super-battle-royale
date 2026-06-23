//! Pickups & power-ups.
//!
//! Power-up pads are authored into maps as `PickupSpawn` (`'p'`) tiles. On match
//! start each pad spawns a replicated pickup; walking a player over one grants a
//! power-up and despawns the pickup, which then respawns at the pad after a delay
//! with the next kind cycled in. Pads are authoritative-only entities, recreated
//! every match like bots (so there's no cross-match state to reset); pickups are
//! replicated so every client sees them appear and vanish in sync.
//!
//! The granted effects are timed buff components defined in [`combat`](super::combat)
//! — kept there so the movement/firing/damage systems that honour them don't
//! depend on this module — except `Heal`, which is applied instantly. Buffs are
//! authoritative-only; their *results* (`NetPos`, `Health`, projectiles) already
//! replicate, so clients need no buff state. Collection feedback rides the shared
//! replicated [`Impact`](super::projectile::Impact) signal, firing identically
//! offline, on the host, and on every client. v1: only players collect pickups.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::combat::{
    DamageBoost, Dead, DoubleShot, Health, QuadShot, RapidFire, SpeedBoost, Zigzag,
};
use super::map::CurrentMap;
use super::net::{
    NetPos, NetworkAppExt, SpawnCommandsExt, SpawnContext, is_authoritative, resolve_spawn_context,
};
use super::player::{PLAYER_SIZE, Player};
use super::projectile::{ImpactKind, spawn_impact};
use super::state::GameState;

#[cfg(feature = "client")]
use super::effects::make_texture;

/// A player collects a pickup when their centres are within this distance plus
/// the player's own half-size.
const PICKUP_RADIUS: f32 = 16.0;
/// Seconds an emptied pad waits before producing its next power-up.
const PICKUP_RESPAWN_DELAY: f32 = 12.0;

/// HP a Heal pickup restores (clamped to the player's max).
const HEAL_AMOUNT: f32 = 50.0;
/// How long each timed buff lasts once collected, in seconds.
const SPEED_DURATION: f32 = 5.0;
const RAPIDFIRE_DURATION: f32 = 6.0;
const DAMAGE_DURATION: f32 = 6.0;
/// Shared duration for the fire-pattern buffs (double / quad / zig-zag).
const PATTERN_DURATION: f32 = 8.0;

/// Side length of a rendered pickup orb (client).
#[cfg(feature = "client")]
const PICKUP_SIZE: f32 = 28.0;
/// Faint ground marker drawn at each pad so empty pads stay visible (client).
/// Non-HDR (≤ 1.0) so it doesn't bloom like the pickups themselves.
#[cfg(feature = "client")]
const PAD_DECAL_COLOR: Color = Color::srgba(0.45, 0.45, 0.55, 0.16);

/// The set of power-ups pads cycle through, in order.
const KINDS: [PickupKind; 7] = [
    PickupKind::Heal,
    PickupKind::Speed,
    PickupKind::RapidFire,
    PickupKind::Damage,
    PickupKind::DoubleShot,
    PickupKind::QuadShot,
    PickupKind::Zigzag,
];

/// What a pickup grants. Doubles as the pickup's marker component (like
/// `Impact(ImpactKind)`) and is replicated so clients draw the matching color.
#[derive(
    Component,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    super::net::Replicated,
)]
pub enum PickupKind {
    #[default]
    Heal,
    Speed,
    RapidFire,
    Damage,
    DoubleShot,
    QuadShot,
    Zigzag,
}

/// Authoritative-only power-up pad: a fixed location holding at most one pickup,
/// which refills [`PICKUP_RESPAWN_DELAY`] after being emptied. Not replicated;
/// tagged `InGame` so `cleanup_ingame` clears it and pads rebuild fresh per match.
#[derive(Component, Debug)]
struct Pad {
    /// Counts down while the pad is empty, then refills it.
    timer: Timer,
    /// Whether the pad currently holds a live pickup.
    occupied: bool,
    /// Index into [`KINDS`] of the *next* power-up this pad will produce.
    cycle: usize,
}

/// Authoritative-only back-link from a pickup to the pad that produced it, so
/// collecting it can free the pad. Mirrors `ProjectileOwner`.
#[derive(Component, Clone, Copy, Debug)]
struct PadOf(Entity);

pub struct PickupPlugin;

impl Plugin for PickupPlugin {
    fn build(&self, app: &mut App) {
        app.register_networked::<PickupKind>();

        // Authoritative: build pads at match start; refill + collect each frame.
        app.add_systems(
            OnEnter(GameState::Playing),
            build_pickup_pads.run_if(is_authoritative),
        )
        .add_systems(
            Update,
            (respawn_pickups, collect_pickups)
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );

        // Client: build the orb texture, draw pad markers + pickups, animate them.
        #[cfg(feature = "client")]
        app.add_systems(Startup, init_pickup_art)
            .add_systems(OnEnter(GameState::Playing), spawn_pad_decals)
            .add_systems(
                Update,
                (attach_pickup_sprite, animate_pickups).run_if(in_state(GameState::Playing)),
            );
    }
}

/// Builds a pad entity per `PickupSpawn` tile and seeds each with its first
/// pickup. Parallels `spawn_bots`; runs only where the simulation is authoritative.
fn build_pickup_pads(mut commands: Commands, map: Res<CurrentMap>, ctx: Option<Res<SpawnContext>>) {
    let ctx = resolve_spawn_context(ctx);
    for (i, &pos) in map.0.pickup_points().iter().enumerate() {
        let len = KINDS.len();
        let pad = commands
            .spawn_ingame((
                Pad {
                    timer: Timer::from_seconds(PICKUP_RESPAWN_DELAY, TimerMode::Once),
                    occupied: true,
                    // The initial pickup is `KINDS[i]`, so the *next* is `i + 1`.
                    cycle: (i + 1) % len,
                },
                NetPos(pos),
            ))
            .id();
        spawn_pickup(&mut commands, ctx, pad, pos, KINDS[i % len]);
    }
}

/// Spawns one replicated pickup at `pos`, linked back to its `pad`.
fn spawn_pickup(
    commands: &mut Commands,
    ctx: SpawnContext,
    pad: Entity,
    pos: Vec2,
    kind: PickupKind,
) {
    commands.spawn_pickup(ctx, (kind, NetPos(pos), PadOf(pad)));
}

/// Refills emptied pads once their respawn timer elapses, cycling the kind.
fn respawn_pickups(
    time: Res<Time>,
    ctx: Option<Res<SpawnContext>>,
    mut commands: Commands,
    mut pads: Query<(Entity, &NetPos, &mut Pad)>,
) {
    let ctx = resolve_spawn_context(ctx);
    for (pad_entity, pos, mut pad) in &mut pads {
        if pad.occupied {
            continue;
        }
        if pad.timer.tick(time.delta()).just_finished() {
            let kind = KINDS[pad.cycle % KINDS.len()];
            pad.cycle = (pad.cycle + 1) % KINDS.len();
            pad.occupied = true;
            spawn_pickup(&mut commands, ctx, pad_entity, pos.0, kind);
        }
    }
}

/// Grants a pickup's power-up to the first live player that touches it, then
/// despawns the pickup and frees its pad (resetting the timer so it can't refill
/// instantly regardless of system order).
#[allow(clippy::type_complexity)]
fn collect_pickups(
    ctx: Option<Res<SpawnContext>>,
    mut commands: Commands,
    pickups: Query<(Entity, &NetPos, &PickupKind, &PadOf)>,
    mut players: Query<(Entity, &NetPos, &mut Health), (With<Player>, Without<Dead>)>,
    mut pads: Query<&mut Pad>,
) {
    let ctx = resolve_spawn_context(ctx);
    let reach = PICKUP_RADIUS + PLAYER_SIZE / 2.0;
    for (pickup, pickup_pos, kind, pad_of) in &pickups {
        for (player, player_pos, mut health) in &mut players {
            if pickup_pos.0.distance(player_pos.0) > reach {
                continue;
            }
            match *kind {
                // Heal is instant; only touch Health here so other kinds don't
                // spuriously mark it changed (and re-replicate).
                PickupKind::Heal => {
                    health.current = (health.current + HEAL_AMOUNT).min(health.max);
                }
                other => grant_buff(&mut commands, player, other),
            }
            spawn_impact(&mut commands, ctx, ImpactKind::Pickup, pickup_pos.0);
            commands.entity(pickup).try_despawn();
            if let Ok(mut pad) = pads.get_mut(pad_of.0) {
                pad.occupied = false;
                pad.timer.reset();
            }
            break;
        }
    }
}

/// Inserts the timed buff component for a (non-Heal) power-up. Re-inserting
/// refreshes the timer, so re-collecting extends an effect rather than stacking it.
fn grant_buff(commands: &mut Commands, player: Entity, kind: PickupKind) {
    let mut player = commands.entity(player);
    match kind {
        PickupKind::Speed => {
            player.insert(SpeedBoost(once(SPEED_DURATION)));
        }
        PickupKind::RapidFire => {
            player.insert(RapidFire(once(RAPIDFIRE_DURATION)));
        }
        PickupKind::Damage => {
            player.insert(DamageBoost(once(DAMAGE_DURATION)));
        }
        PickupKind::DoubleShot => {
            player.insert(DoubleShot(once(PATTERN_DURATION)));
        }
        PickupKind::QuadShot => {
            player.insert(QuadShot(once(PATTERN_DURATION)));
        }
        PickupKind::Zigzag => {
            player.insert(Zigzag(once(PATTERN_DURATION)));
        }
        // Handled instantly by the caller.
        PickupKind::Heal => {}
    }
}

fn once(secs: f32) -> Timer {
    Timer::from_seconds(secs, TimerMode::Once)
}

/// Resolution of each generated glyph texture.
#[cfg(feature = "client")]
const GLYPH_SIZE: usize = 64;

/// Per-kind procedurally-generated glyph textures (white masks, tinted at draw
/// time), indexed by [`PickupKind::glyph_index`], plus the soft halo used for the
/// ground pad markers.
#[cfg(feature = "client")]
#[derive(Resource)]
struct PickupArt {
    glyphs: [Handle<Image>; 7],
    halo: Handle<Image>,
}

#[cfg(feature = "client")]
impl PickupKind {
    /// Stable index into [`PickupArt::glyphs`]. Order is arbitrary but must match
    /// the array built in [`init_pickup_art`].
    fn glyph_index(self) -> usize {
        match self {
            PickupKind::Heal => 0,
            PickupKind::Speed => 1,
            PickupKind::RapidFire => 2,
            PickupKind::Damage => 3,
            PickupKind::DoubleShot => 4,
            PickupKind::QuadShot => 5,
            PickupKind::Zigzag => 6,
        }
    }
}

/// Builds the distinct power-up glyph textures + the pad halo once at startup.
#[cfg(feature = "client")]
fn init_pickup_art(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    // In `glyph_index` order.
    let kinds = [
        PickupKind::Heal,
        PickupKind::Speed,
        PickupKind::RapidFire,
        PickupKind::Damage,
        PickupKind::DoubleShot,
        PickupKind::QuadShot,
        PickupKind::Zigzag,
    ];
    let glyphs =
        kinds.map(|kind| images.add(make_texture(GLYPH_SIZE, move |p| glyph_alpha(kind, p))));
    let halo = images.add(make_texture(48, |p| {
        (1.0 - p.length()).clamp(0.0, 1.0).powf(1.5)
    }));
    commands.insert_resource(PickupArt { glyphs, halo });
}

/// Gives any pickup that lacks a sprite (freshly spawned or replicated in) its
/// kind-specific glowing glyph. Mirrors `attach_bot_sprite`.
#[cfg(feature = "client")]
fn attach_pickup_sprite(
    mut commands: Commands,
    art: Option<Res<PickupArt>>,
    query: Query<(Entity, &NetPos, &PickupKind), Without<Sprite>>,
) {
    let Some(art) = art else {
        return;
    };
    for (entity, pos, kind) in &query {
        commands.entity(entity).insert((
            Sprite {
                image: art.glyphs[kind.glyph_index()].clone(),
                color: pickup_glow(*kind),
                custom_size: Some(Vec2::splat(PICKUP_SIZE)),
                ..default()
            },
            // Above the floor/pad decal, below players (world z 10).
            Transform::from_xyz(pos.0.x, pos.0.y, 5.0),
            super::InGame,
        ));
    }
}

/// Makes pickups feel alive: a gentle size "breath" plus a slow spin, with a
/// per-kind phase so they don't pulse in unison. Only `custom_size` and the
/// rotation are touched, so this never fights `sync_netpos_to_transform` (which
/// owns the translation).
#[cfg(feature = "client")]
fn animate_pickups(time: Res<Time>, mut query: Query<(&mut Sprite, &mut Transform, &PickupKind)>) {
    let t = time.elapsed_secs();
    for (mut sprite, mut transform, kind) in &mut query {
        let phase = kind.glyph_index() as f32;
        let pulse = 1.0 + 0.1 * (t * 3.0 + phase).sin();
        sprite.custom_size = Some(Vec2::splat(PICKUP_SIZE * pulse));
        transform.rotation = Quat::from_rotation_z(t * 0.6);
    }
}

/// Draws a faint ground halo at each pad so players learn where pickups appear,
/// even while a pad is empty. Reads the (client-resident) `CurrentMap`.
#[cfg(feature = "client")]
fn spawn_pad_decals(mut commands: Commands, art: Option<Res<PickupArt>>, map: Res<CurrentMap>) {
    let Some(art) = art else {
        return;
    };
    for &pos in map.0.pickup_points() {
        commands.spawn_ingame((
            Sprite {
                image: art.halo.clone(),
                color: PAD_DECAL_COLOR,
                custom_size: Some(Vec2::splat(PICKUP_SIZE * 1.7)),
                ..default()
            },
            // On the ground (above floor/walls, beneath players), like shockwaves.
            Transform::from_xyz(pos.x, pos.y, 2.0),
        ));
    }
}

/// HDR (linear > 1.0) glow color per kind, so pickups bloom and read at a glance.
/// Chosen to be mutually distinct around the wheel.
#[cfg(feature = "client")]
fn pickup_glow(kind: PickupKind) -> Color {
    match kind {
        PickupKind::Heal => Color::linear_rgb(1.2, 8.0, 2.2), // green
        PickupKind::Speed => Color::linear_rgb(1.2, 7.0, 8.0), // cyan
        PickupKind::RapidFire => Color::linear_rgb(8.0, 6.0, 1.2), // gold
        PickupKind::Damage => Color::linear_rgb(8.0, 1.6, 1.6), // red
        PickupKind::DoubleShot => Color::linear_rgb(8.0, 3.5, 1.0), // orange
        PickupKind::QuadShot => Color::linear_rgb(6.0, 1.4, 8.0), // magenta
        PickupKind::Zigzag => Color::linear_rgb(4.5, 6.5, 9.0), // electric blue-white
    }
}

// --- Procedural glyphs -----------------------------------------------------
//
// Each glyph is an alpha mask over normalized coords `p ∈ [-1, 1]²`. A faint
// radial halo is baked into every glyph so the pickup reads as a glowing orb of
// energy with a bright signature shape on top once tinted and bloomed.

/// Alpha for `kind`'s glyph at normalized point `p`.
#[cfg(feature = "client")]
fn glyph_alpha(kind: PickupKind, p: Vec2) -> f32 {
    let shape = match kind {
        // Medic cross.
        PickupKind::Heal => stroke(
            p,
            &[
                (Vec2::new(0.0, -0.62), Vec2::new(0.0, 0.62)),
                (Vec2::new(-0.62, 0.0), Vec2::new(0.62, 0.0)),
            ],
            0.27,
        ),
        // Stacked speed chevrons.
        PickupKind::Speed => stroke(
            p,
            &[
                (Vec2::new(-0.55, -0.6), Vec2::new(0.0, -0.15)),
                (Vec2::new(0.55, -0.6), Vec2::new(0.0, -0.15)),
                (Vec2::new(-0.55, 0.05), Vec2::new(0.0, 0.5)),
                (Vec2::new(0.55, 0.05), Vec2::new(0.0, 0.5)),
            ],
            0.16,
        ),
        // Rapid concentric rings.
        PickupKind::RapidFire => rings(p),
        // Eight-spike damage sunburst.
        PickupKind::Damage => star(p, 8.0, 0.18, 0.95),
        // Opposing two-way arrows.
        PickupKind::DoubleShot => stroke(
            p,
            &[
                (Vec2::new(0.0, 0.1), Vec2::new(0.0, 0.78)),
                (Vec2::new(-0.3, 0.45), Vec2::new(0.0, 0.8)),
                (Vec2::new(0.3, 0.45), Vec2::new(0.0, 0.8)),
                (Vec2::new(0.0, -0.1), Vec2::new(0.0, -0.78)),
                (Vec2::new(-0.3, -0.45), Vec2::new(0.0, -0.8)),
                (Vec2::new(0.3, -0.45), Vec2::new(0.0, -0.8)),
            ],
            0.14,
        ),
        // Sharp four-pointed star (the cross of fire).
        PickupKind::QuadShot => star(p, 4.0, 0.1, 0.95),
        // Lightning bolt.
        PickupKind::Zigzag => stroke(
            p,
            &[
                (Vec2::new(0.18, 0.82), Vec2::new(-0.28, 0.12)),
                (Vec2::new(-0.28, 0.12), Vec2::new(0.12, 0.08)),
                (Vec2::new(0.12, 0.08), Vec2::new(-0.2, -0.82)),
            ],
            0.13,
        ),
    };
    // Bake a soft halo so even the thin glyphs read as an energy orb.
    let halo = (1.0 - p.length()).clamp(0.0, 1.0).powf(2.4) * 0.22;
    shape.max(halo).min(1.0)
}

/// Distance from `p` to the segment `a`–`b`.
#[cfg(feature = "client")]
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Glowing intensity near a set of line segments: bright on the line, soft falloff.
#[cfg(feature = "client")]
fn stroke(p: Vec2, segments: &[(Vec2, Vec2)], thickness: f32) -> f32 {
    let nearest = segments
        .iter()
        .map(|&(a, b)| seg_dist(p, a, b))
        .fold(f32::INFINITY, f32::min);
    (1.0 - nearest / thickness).clamp(0.0, 1.0).powf(1.3)
}

/// A filled star with `points` spikes, bright at the centre and fading at the
/// spike tips (between radii `inner` and `outer`).
#[cfg(feature = "client")]
fn star(p: Vec2, points: f32, inner: f32, outer: f32) -> f32 {
    let angle = p.y.atan2(p.x);
    let edge = inner + (outer - inner) * ((angle * points).cos() * 0.5 + 0.5);
    (1.0 - p.length() / edge.max(1e-3))
        .clamp(0.0, 1.0)
        .powf(0.8)
}

/// Three soft concentric rings (a rapid-pulse signature).
#[cfg(feature = "client")]
fn rings(p: Vec2) -> f32 {
    let r = p.length();
    let band = |center: f32| {
        let d = (r - center) / 0.07;
        (-d * d).exp()
    };
    (band(0.3) + band(0.6) + band(0.92)).min(1.0)
}

#[cfg(test)]
mod tests {
    // `super::*` re-exports the buff/types pickup already imports (Health,
    // SpeedBoost, DamageBoost, NetPos, Player, GameState, the bevy prelude, ...);
    // only items pickup doesn't import are added explicitly, to avoid clashes.
    use super::*;
    use crate::game::combat::CombatPlugin;
    use crate::game::map::TileMap;
    use crate::game::net::NetRole;
    use crate::game::projectile::{Projectile, ProjectileOwner};
    use crate::game::state::GameState;
    use bevy::state::app::StatesPlugin;
    use std::time::Duration;

    /// A headless authoritative app running the combat + (authoritative) pickup
    /// systems only — no client render systems, so no assets are needed.
    fn test_app(map_text: &str) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin, CombatPlugin));
        app.insert_resource(NetRole::Server);
        app.insert_resource(CurrentMap(TileMap::parse(map_text)));
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f32(1.0 / 60.0),
        ));
        app.add_systems(
            OnEnter(GameState::Playing),
            build_pickup_pads.run_if(is_authoritative),
        )
        .add_systems(
            Update,
            (respawn_pickups, collect_pickups)
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );
        app.insert_state(GameState::Playing);
        app
    }

    fn count_pickups(app: &mut App) -> usize {
        let mut q = app.world_mut().query::<&PickupKind>();
        q.iter(app.world()).count()
    }

    /// Drops a pickup of `kind` on `pos`; the dummy pad link is harmless (the
    /// freed-pad lookup just misses).
    fn drop_pickup(app: &mut App, kind: PickupKind, pos: Vec2) {
        let pad = app.world_mut().spawn_empty().id();
        app.world_mut().spawn((kind, NetPos(pos), PadOf(pad)));
    }

    #[test]
    fn heal_restores_health_and_clamps_to_max() {
        let mut app = test_app("wsw");
        let player = app.world_mut().spawn((Player, NetPos(Vec2::ZERO))).id();
        app.update(); // ensure_health → full (2.0 / 2.0)
        app.world_mut().get_mut::<Health>(player).unwrap().current = 1.0;

        drop_pickup(&mut app, PickupKind::Heal, Vec2::ZERO);
        app.update();
        assert_eq!(
            app.world().get::<Health>(player).unwrap().current,
            2.0,
            "heal should restore to max HP"
        );

        // A second heal must clamp to the player's max.
        drop_pickup(&mut app, PickupKind::Heal, Vec2::ZERO);
        app.update();
        let health = app.world().get::<Health>(player).unwrap();
        assert_eq!(health.current, health.max, "heal must clamp to max HP");
    }

    #[test]
    fn collecting_speed_grants_a_speed_buff() {
        let mut app = test_app("wsw");
        let player = app.world_mut().spawn((Player, NetPos(Vec2::ZERO))).id();
        app.update();
        drop_pickup(&mut app, PickupKind::Speed, Vec2::ZERO);
        app.update();
        assert!(
            app.world().get::<SpeedBoost>(player).is_some(),
            "collecting a Speed pickup should grant a SpeedBoost"
        );
    }

    #[test]
    fn timed_buffs_expire_after_their_duration() {
        let mut app = test_app("wsw");
        let player = app.world_mut().spawn((Player, NetPos(Vec2::ZERO))).id();
        app.update();
        // A short-lived buff that should be ticked away by `tick_buff`.
        app.world_mut()
            .entity_mut(player)
            .insert(SpeedBoost(Timer::from_seconds(0.1, TimerMode::Once)));
        for _ in 0..20 {
            app.update(); // ~0.33s at 1/60 per step
        }
        assert!(
            app.world().get::<SpeedBoost>(player).is_none(),
            "an expired buff should remove itself"
        );
    }

    #[test]
    fn damage_boost_doubles_shot_damage() {
        let mut app = test_app("wsw");
        let shooter = app.world_mut().spawn((Player, NetPos(Vec2::ZERO))).id();
        // Give the target enough health that a doubled 1-damage shot is visible.
        let target = app
            .world_mut()
            .spawn((
                Player,
                NetPos(Vec2::ZERO),
                Health {
                    current: 4.0,
                    max: 4.0,
                },
            ))
            .id();
        app.update();
        app.world_mut()
            .entity_mut(shooter)
            .insert(DamageBoost(Timer::from_seconds(5.0, TimerMode::Once)));

        app.world_mut()
            .spawn((Projectile, ProjectileOwner(shooter), NetPos(Vec2::ZERO)));
        app.update();
        // Base 1 × 2 = 2 damage.
        assert_eq!(app.world().get::<Health>(target).unwrap().current, 2.0);
    }

    #[test]
    fn collected_pickup_frees_its_pad_and_respawns_after_delay() {
        // A one-row map with a single pickup pad ('p').
        let mut app = test_app("wspw");
        app.update(); // Startup + OnEnter → one pad seeded with one pickup
        assert_eq!(count_pickups(&mut app), 1, "a pad should seed one pickup");

        let pos = app.world().resource::<CurrentMap>().0.pickup_points()[0];
        let player = app.world_mut().spawn((Player, NetPos(pos))).id();
        // A couple of frames so ensure_health runs before collection resolves.
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            count_pickups(&mut app),
            0,
            "walking onto the pad collects it"
        );

        // Remove the player so it can't instantly re-collect the respawn.
        app.world_mut().entity_mut(player).despawn();
        // Advance past the respawn delay (12s at 1/60 ≈ 720 frames).
        for _ in 0..760 {
            app.update();
        }
        assert_eq!(
            count_pickups(&mut app),
            1,
            "the pad refills after the delay"
        );
    }
}
