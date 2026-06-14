//! Shooting: straight-flying, slowly-falling projectiles.
//!
//! Press Space to fire a shot in the player's last-moved direction. The shot
//! travels at a constant horizontal speed while its altitude ([`Height`]) sinks
//! under gentle gravity — no upward lob — and it "crashes" (despawns) once the
//! altitude reaches the ground. PvP damage on contact lives in `combat.rs`.
//!
//! Like every dynamic entity, a projectile's ground position lives in [`NetPos`]
//! and replicates; its altitude replicates via [`Height`] so clients can draw the
//! descent; its velocity is server/sim-only. Firing and motion run on the
//! authoritative side (offline + server); rendering runs on the client.

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use serde::{Deserialize, Serialize};

use super::bot::{Bot, BotIntent};
use super::combat::RapidFire;
use super::map::{ArenaBounds, CurrentMap};
use super::net::{NetPos, is_authoritative};
use super::player::{Player, PlayerColor, PlayerIntent};
use super::state::GameState;

#[cfg(feature = "client")]
use super::combat::{Dead, DoubleShot, QuadShot, Zigzag};
#[cfg(feature = "client")]
use super::net::is_offline;

/// Constant horizontal speed of a shot, in world units per second.
const PROJECTILE_SPEED: f32 = 360.0;
/// Downward acceleration applied to a shot's altitude. Deliberately small so the
/// shot flies nearly straight and sinks gradually.
const GRAVITY: f32 = 60.0;
/// Altitude a shot starts at (vertical velocity starts at zero). Kept low so the
/// shot leaves around the player's body rather than above their head.
const INITIAL_HEIGHT: f32 = 12.0;
/// A shot crashes once its altitude reaches this "ground" level.
const GROUND_LEVEL: f32 = 0.0;
/// Minimum seconds between shots from one player.
const FIRE_COOLDOWN: f32 = 0.35;
/// How much faster the fire-rate cooldown ticks while a player holds [`RapidFire`].
const RAPIDFIRE_FACTOR: f32 = 2.5;
/// Peak sideways deflection (radians) of a zig-zagging shot's velocity.
const ZIGZAG_AMPLITUDE: f32 = 0.7;
/// Angular frequency (radians/second) of a zig-zagging shot's weave.
const ZIGZAG_FREQUENCY: f32 = 12.0;
/// Collision radius of a shot, used for PvP hit detection (shared with the
/// server, so it is not gated behind the `client` feature).
pub const PROJECTILE_RADIUS: f32 = 5.0;
/// How long an [`Impact`] marker entity lives — long enough to replicate to
/// clients before being cleaned up.
const IMPACT_LIFETIME: f32 = 0.3;

/// Side length of the (square) projectile sprite. Client-only (rendering).
#[cfg(feature = "client")]
const PROJECTILE_SIZE: f32 = PROJECTILE_RADIUS * 2.0;

#[cfg(feature = "client")]
const SHADOW_COLOR: Color = Color::srgba(0.0, 0.0, 0.0, 0.35);

// Glowing motion trail: each frame drops a fading segment behind the shot.
#[cfg(feature = "client")]
const TRAIL_LIFETIME: f32 = 0.22;
#[cfg(feature = "client")]
const TRAIL_SIZE: f32 = PROJECTILE_SIZE * 0.85;

// Sound effects, all played non-spatially (the whole arena is on screen).
#[cfg(feature = "client")]
const SHOOT_SOUND: &str = "soundfx/sfx_shoot_heavy.mp3";
#[cfg(feature = "client")]
const HIT_GROUND_SOUND: &str = "soundfx/sfx_hit_ground.mp3";
#[cfg(feature = "client")]
const HIT_OBJECT_SOUND: &str = "soundfx/sfx_hit_object.mp3";
#[cfg(feature = "client")]
const SFX_VOLUME: f32 = 0.6;

/// Replicated marker for a shot.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Projectile;

/// Replicated altitude of a shot above the ground. The renderer offsets the
/// sprite upward by this; simulation lowers it under gravity until it crashes.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Height(pub f32);

/// The firing player's color, replicated so the shot and its trail glow to match.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct ShotColor(pub PlayerColor);

/// Server/sim-only velocity of a shot. Not replicated (clients only need the
/// resulting [`NetPos`]/[`Height`]).
#[derive(Component, Clone, Copy, Debug)]
pub struct ProjectileVelocity {
    pub horizontal: Vec2,
    pub vertical: f32,
}

/// The player who fired a shot, so it never damages its owner. Server/sim-only.
#[derive(Component, Clone, Copy, Debug)]
pub struct ProjectileOwner(pub Entity);

/// A player's last-moved direction, used as the firing direction. Server/sim-only
/// (defaults to "up" until the player first moves).
#[derive(Component, Clone, Copy, Debug)]
pub struct Facing(pub Vec2);

/// Per-player fire-rate limiter. Server/sim-only.
#[derive(Component, Debug)]
pub struct FireCooldown(pub Timer);

/// Server/sim-only: makes a shot weave from side to side. `elapsed` accumulates
/// flight time; its initial value is seeded from the launch angle so several
/// shots fired at once (a quad-burst) weave out of phase rather than in lockstep.
/// Clients render the weave for free via the replicated [`NetPos`].
#[derive(Component, Clone, Copy, Debug)]
pub struct ZigzagMotion {
    elapsed: f32,
}

/// Firing modifiers a shot inherits from its shooter's active power-ups: how many
/// directions to fire in (1, 2 or 4) and whether the shots weave. Built at each
/// fire site from the shooter's buff components and passed into [`try_fire`].
#[derive(Clone, Copy, Debug)]
pub struct ShotMods {
    pub directions: u8,
    pub zigzag: bool,
}

impl Default for ShotMods {
    fn default() -> Self {
        Self {
            directions: 1,
            zigzag: false,
        }
    }
}

impl ShotMods {
    /// A single straight shot — the default for shooters with no fire-pattern buffs.
    pub fn single() -> Self {
        Self::default()
    }

    /// Builds the modifiers from a shooter's active fire-pattern buffs. Quad
    /// (four-way) supersedes Double (two-way) when both are held.
    pub fn from_buffs(double: bool, quad: bool, zigzag: bool) -> Self {
        let directions = if quad {
            4
        } else if double {
            2
        } else {
            1
        };
        Self { directions, zigzag }
    }
}

/// Client-only marker for the ground shadow drawn beneath a shot.
#[cfg(feature = "client")]
#[derive(Component)]
pub struct ProjectileShadow;

/// Client-only fading segment of a shot's glowing trail. Holds its own lifetime
/// and the (full-brightness) glow color to fade from.
#[cfg(feature = "client")]
#[derive(Component)]
struct TrailSegment {
    timer: Timer,
    glow: Color,
}

/// What a shot struck when it ended, used to pick the impact sound.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImpactKind {
    /// Crashed into the ground.
    #[default]
    Ground,
    /// Struck a player.
    Object,
    /// A power-up was collected.
    Pickup,
}

/// A short-lived, replicated marker spawned where a shot ended. Clients play the
/// matching sound when one appears; the authoritative side cleans it up. Using a
/// replicated entity (rather than a one-off network message) makes the audio cue
/// fire identically offline, on the server, and on connected clients.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Impact(pub ImpactKind);

/// Server/sim-only lifetime for an [`Impact`] marker.
#[derive(Component)]
struct ImpactLifetime(Timer);

pub struct ProjectilePlugin;

impl Plugin for ProjectilePlugin {
    fn build(&self, app: &mut App) {
        // Firing and motion run wherever we're authoritative (server + offline).
        app.add_systems(
            Update,
            (
                ensure_shooting_components,
                update_facing,
                tick_cooldowns,
                simulate_projectiles,
                tick_impacts,
            )
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );

        // Local input + rendering + sound live only in the windowed client.
        #[cfg(feature = "client")]
        {
            app.add_systems(
                Update,
                (
                    offline_shoot.run_if(is_offline),
                    attach_projectile_sprite,
                    play_shoot_sound,
                    play_impact_sounds,
                    spawn_projectile_trail,
                    fade_trail,
                )
                    .run_if(in_state(GameState::Playing)),
            );
            app.add_systems(
                PostUpdate,
                render_projectiles.run_if(in_state(GameState::Playing)),
            );
        }
    }
}

/// Gives authoritative players and bots their shooting components
/// (idempotent, so it covers offline spawns, server-spawned players, and
/// replicated-in bots).
#[allow(clippy::type_complexity)]
pub(crate) fn ensure_shooting_components(
    mut commands: Commands,
    entities: Query<Entity, (Or<(With<Player>, With<Bot>)>, Without<FireCooldown>)>,
) {
    for entity in &entities {
        let mut timer = Timer::from_seconds(FIRE_COOLDOWN, TimerMode::Once);
        // Start "ready" so the first shot fires immediately.
        timer.finish();
        commands
            .entity(entity)
            .insert((Facing(Vec2::Y), FireCooldown(timer)));
    }
}

/// Tracks each player's last-moved direction and each bot's current intent
/// as their firing direction.
#[allow(clippy::type_complexity)]
fn update_facing(
    mut actors: ParamSet<(
        Query<(&PlayerIntent, &mut Facing), With<Player>>,
        Query<(&BotIntent, &mut Facing), With<Bot>>,
    )>,
) {
    for (intent, mut facing) in actors.p0() {
        if intent.0 != Vec2::ZERO {
            facing.0 = intent.0.normalize_or_zero();
        }
    }
    for (intent, mut facing) in actors.p1() {
        if intent.0 != Vec2::ZERO {
            facing.0 = intent.0.normalize_or_zero();
        }
    }
}

pub(crate) fn tick_cooldowns(
    time: Res<Time>,
    mut query: Query<(&mut FireCooldown, Option<&RapidFire>)>,
) {
    for (mut cooldown, rapid) in &mut query {
        // RapidFire just advances the cooldown faster, so it self-cleans when the
        // buff component is removed — no stored base duration to restore.
        let step = if rapid.is_some() {
            time.delta().mul_f32(RAPIDFIRE_FACTOR)
        } else {
            time.delta()
        };
        cooldown.0.tick(step);
    }
}

/// Moves shots forward, sinks them under gravity, and despawns them when they
/// crash into the ground or leave the arena.
#[allow(clippy::type_complexity)]
fn simulate_projectiles(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    map: Res<CurrentMap>,
    mut commands: Commands,
    mut query: Query<
        (
            Entity,
            &mut NetPos,
            &mut Height,
            &mut ProjectileVelocity,
            Option<&mut ZigzagMotion>,
        ),
        With<Projectile>,
    >,
) {
    let dt = time.delta_secs();
    for (entity, mut pos, mut height, mut velocity, zigzag) in &mut query {
        // A zig-zagging shot rotates its (constant) base velocity by an
        // oscillating angle each frame; rotating a fresh copy avoids drift.
        let horizontal = match zigzag {
            Some(mut motion) => {
                motion.elapsed += dt;
                let angle = ZIGZAG_AMPLITUDE * (ZIGZAG_FREQUENCY * motion.elapsed).sin();
                Vec2::from_angle(angle).rotate(velocity.horizontal)
            }
            None => velocity.horizontal,
        };
        pos.0 += horizontal * dt;
        velocity.vertical -= GRAVITY * dt;
        height.0 += velocity.vertical * dt;

        let p = pos.0;
        let out_of_bounds =
            p.x < bounds.min.x || p.x > bounds.max.x || p.y < bounds.min.y || p.y > bounds.max.y;
        let hit_wall = map.0.circle_intersects_wall(p, PROJECTILE_RADIUS);
        if height.0 <= GROUND_LEVEL || hit_wall {
            spawn_impact(&mut commands, ImpactKind::Ground, pos.0);
            commands.entity(entity).despawn();
        } else if out_of_bounds {
            commands.entity(entity).despawn();
        }
    }
}

/// Spawns a replicated impact marker (carrying where it happened) so clients can
/// play the matching sound and spawn visual effects there.
pub(crate) fn spawn_impact(commands: &mut Commands, kind: ImpactKind, position: Vec2) {
    commands.spawn((
        Impact(kind),
        NetPos(position),
        ImpactLifetime(Timer::from_seconds(IMPACT_LIFETIME, TimerMode::Once)),
        Replicated,
    ));
}

/// Cleans up impact markers once they've had time to replicate to clients.
fn tick_impacts(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut ImpactLifetime)>,
) {
    for (entity, mut lifetime) in &mut query {
        if lifetime.0.tick(time.delta()).just_finished() {
            commands.entity(entity).despawn();
        }
    }
}

/// Fires from `origin` along `facing` if the cooldown has elapsed, honouring the
/// shooter's fire-pattern power-ups via `mods`. Shared by offline input, the
/// server's network handler, and bot AI. One cooldown reset covers the whole
/// burst, so multi-shot adds bullets without raising the fire rate.
pub(crate) fn try_fire(
    commands: &mut Commands,
    owner: Entity,
    color: PlayerColor,
    origin: &NetPos,
    facing: &Facing,
    cooldown: &mut FireCooldown,
    mods: ShotMods,
) {
    if !cooldown.0.is_finished() {
        return;
    }
    cooldown.0.reset();

    let forward = facing.0.normalize_or_zero();
    match mods.directions {
        // Forward + backward.
        2 => {
            spawn_projectile(commands, owner, color, origin.0, forward, mods.zigzag);
            spawn_projectile(commands, owner, color, origin.0, -forward, mods.zigzag);
        }
        // A four-way cross around the facing direction.
        4 => {
            let side = forward.perp();
            for dir in [forward, side, -forward, -side] {
                spawn_projectile(commands, owner, color, origin.0, dir, mods.zigzag);
            }
        }
        // A single straight shot.
        _ => spawn_projectile(commands, owner, color, origin.0, forward, mods.zigzag),
    }
}

fn spawn_projectile(
    commands: &mut Commands,
    owner: Entity,
    color: PlayerColor,
    origin: Vec2,
    direction: Vec2,
    zigzag: bool,
) {
    let dir = direction.normalize_or_zero();
    let mut shot = commands.spawn((
        Projectile,
        ProjectileOwner(owner),
        ShotColor(color),
        NetPos(origin),
        Height(INITIAL_HEIGHT),
        ProjectileVelocity {
            horizontal: dir * PROJECTILE_SPEED,
            vertical: 0.0,
        },
        Replicated,
        // Authoritative-side tag so leftover in-flight shots are cleared on a map
        // switch. Clients render the replicated shot and let replicon despawn it.
        super::InGame,
    ));
    if zigzag {
        // Seed the weave phase from the launch angle so a multi-shot burst fans out.
        shot.insert(ZigzagMotion {
            elapsed: dir.to_angle(),
        });
    }
}

/// Offline single-player: fire the local player on Space.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn offline_shoot(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut players: Query<
        (
            Entity,
            &NetPos,
            &Facing,
            &mut FireCooldown,
            &PlayerColor,
            Option<&DoubleShot>,
            Option<&QuadShot>,
            Option<&Zigzag>,
        ),
        (With<Player>, Without<Dead>),
    >,
) {
    if !keys.just_pressed(KeyCode::Space) {
        return;
    }
    for (entity, pos, facing, mut cooldown, color, double, quad, zigzag) in &mut players {
        let mods = ShotMods::from_buffs(double.is_some(), quad.is_some(), zigzag.is_some());
        try_fire(
            &mut commands,
            entity,
            *color,
            pos,
            facing,
            &mut cooldown,
            mods,
        );
    }
}

/// A bright HDR (linear > 1.0) version of a player's color, so the shot blooms.
#[cfg(feature = "client")]
pub(crate) fn shot_glow(color: PlayerColor) -> Color {
    match color {
        PlayerColor::Red => Color::linear_rgb(8.0, 1.5, 1.5),
        PlayerColor::Blue => Color::linear_rgb(1.5, 3.0, 8.0),
        PlayerColor::Green => Color::linear_rgb(1.5, 7.0, 2.0),
        PlayerColor::Orange => Color::linear_rgb(8.0, 3.5, 1.0),
        PlayerColor::Purple => Color::linear_rgb(5.0, 1.5, 8.0),
        PlayerColor::Yellow => Color::linear_rgb(7.0, 6.0, 1.5),
    }
}

/// Gives a freshly spawned/replicated shot its sprite plus a ground shadow.
/// The shadow is a child, so it despawns recursively with the projectile.
#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn attach_projectile_sprite(
    mut commands: Commands,
    query: Query<(Entity, &NetPos, &Height, &ShotColor), (With<Projectile>, Without<Sprite>)>,
) {
    for (entity, pos, height, color) in &query {
        commands
            .entity(entity)
            .insert((
                Sprite {
                    color: shot_glow(color.0),
                    custom_size: Some(Vec2::splat(PROJECTILE_SIZE)),
                    ..default()
                },
                Transform::from_xyz(pos.0.x, pos.0.y + height.0, 20.0),
            ))
            .with_children(|parent| {
                // Local offset cancels the parent's altitude so the shadow stays
                // pinned to the ground; z lands it just under players (world z 9).
                parent.spawn((
                    ProjectileShadow,
                    Sprite {
                        color: SHADOW_COLOR,
                        custom_size: Some(Vec2::new(PROJECTILE_SIZE, PROJECTILE_SIZE * 0.5)),
                        ..default()
                    },
                    Transform::from_xyz(0.0, -height.0, -11.0),
                ));
            });
    }
}

/// Positions each shot at `(NetPos.x, NetPos.y + Height)` and keeps its shadow on
/// the ground. Runs instead of `sync_netpos_to_transform` for projectiles.
#[cfg(feature = "client")]
fn render_projectiles(
    projectiles: Query<(Entity, &NetPos, &Height, Option<&Children>), With<Projectile>>,
    mut transforms: Query<&mut Transform>,
    shadows: Query<(), With<ProjectileShadow>>,
) {
    for (entity, pos, height, children) in &projectiles {
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.translation.x = pos.0.x;
            transform.translation.y = pos.0.y + height.0;
        }
        if let Some(children) = children {
            for &child in children {
                if shadows.contains(child)
                    && let Ok(mut shadow) = transforms.get_mut(child)
                {
                    shadow.translation.y = -height.0;
                }
            }
        }
    }
}

/// Spawns a one-shot, non-spatial sound that despawns when it finishes.
#[cfg(feature = "client")]
fn play_sound(commands: &mut Commands, asset_server: &AssetServer, path: &'static str) {
    commands.spawn((
        AudioPlayer::new(asset_server.load(path)),
        PlaybackSettings {
            mode: bevy::audio::PlaybackMode::Despawn,
            volume: bevy::audio::Volume::Linear(SFX_VOLUME),
            ..default()
        },
        super::InGame,
    ));
}

/// Plays the shoot sound once for each newly spawned (or replicated-in) shot.
#[cfg(feature = "client")]
fn play_shoot_sound(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    new_shots: Query<Entity, Added<Projectile>>,
) {
    for _ in &new_shots {
        play_sound(&mut commands, &asset_server, SHOOT_SOUND);
    }
}

/// Plays the matching sound when an impact marker appears.
#[cfg(feature = "client")]
fn play_impact_sounds(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    impacts: Query<&Impact, Added<Impact>>,
) {
    for impact in &impacts {
        let path = match impact.0 {
            ImpactKind::Ground => HIT_GROUND_SOUND,
            ImpactKind::Object => HIT_OBJECT_SOUND,
            // Pickups get a visual pop only for now (no fitting chime asset yet).
            ImpactKind::Pickup => continue,
        };
        play_sound(&mut commands, &asset_server, path);
    }
}

/// Drops a glowing trail segment at each shot's current position every frame.
/// Segments are independent entities, so they linger in place to form the tail.
#[cfg(feature = "client")]
fn spawn_projectile_trail(
    mut commands: Commands,
    projectiles: Query<(&NetPos, &Height, &ShotColor), With<Projectile>>,
) {
    for (pos, height, color) in &projectiles {
        let glow = shot_glow(color.0);
        commands.spawn((
            TrailSegment {
                timer: Timer::from_seconds(TRAIL_LIFETIME, TimerMode::Once),
                glow,
            },
            Sprite {
                color: glow,
                custom_size: Some(Vec2::splat(TRAIL_SIZE)),
                ..default()
            },
            // Just behind the shot (z 19) but still above players/shadow.
            Transform::from_xyz(pos.0.x, pos.0.y + height.0, 19.0),
            super::InGame,
        ));
    }
}

/// Fades trail segments out (translucent + smaller) and despawns expired ones.
/// Fading the alpha keeps the glow color but lowers its rendered brightness, so
/// the tail tapers off without leaving dark squares.
#[cfg(feature = "client")]
fn fade_trail(
    time: Res<Time>,
    mut commands: Commands,
    mut segments: Query<(Entity, &mut TrailSegment, &mut Sprite)>,
) {
    for (entity, mut segment, mut sprite) in &mut segments {
        if segment.timer.tick(time.delta()).just_finished() {
            commands.entity(entity).despawn();
            continue;
        }
        let f = 1.0 - segment.timer.fraction();
        sprite.color = segment.glow.with_alpha(f);
        sprite.custom_size = Some(Vec2::splat(TRAIL_SIZE * f));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::map::TileMap;
    use std::time::Duration;

    /// Fires once with `directions` fire-pattern and returns how many projectiles
    /// were spawned. A "ready" cooldown lets the single shot through.
    fn projectiles_fired(directions: u8) -> usize {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let mut ready = Timer::from_seconds(FIRE_COOLDOWN, TimerMode::Once);
        ready.finish();
        let owner = app
            .world_mut()
            .spawn((NetPos(Vec2::ZERO), Facing(Vec2::Y), FireCooldown(ready)))
            .id();
        app.add_systems(
            Update,
            move |mut commands: Commands,
                  mut shooter: Query<(&NetPos, &Facing, &mut FireCooldown)>| {
                if let Ok((pos, facing, mut cooldown)) = shooter.get_mut(owner) {
                    try_fire(
                        &mut commands,
                        owner,
                        PlayerColor::Blue,
                        pos,
                        facing,
                        &mut cooldown,
                        ShotMods {
                            directions,
                            zigzag: false,
                        },
                    );
                }
            },
        );
        app.update();
        let mut q = app.world_mut().query::<&Projectile>();
        q.iter(app.world()).count()
    }

    #[test]
    fn fire_patterns_spawn_the_expected_shot_count() {
        assert_eq!(projectiles_fired(1), 1, "single shot");
        assert_eq!(
            projectiles_fired(2),
            2,
            "double shot fires forward + backward"
        );
        assert_eq!(projectiles_fired(4), 4, "quad shot fires a four-way cross");
    }

    #[test]
    fn zigzag_projectile_weaves_sideways_while_advancing() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f32(1.0 / 60.0),
        ));
        // An open, wall-free map and huge bounds so the shot never despawns.
        app.insert_resource(CurrentMap(TileMap::parse("xxxxxxxxxx")));
        app.insert_resource(ArenaBounds {
            min: Vec2::splat(-10_000.0),
            max: Vec2::splat(10_000.0),
        });
        app.add_systems(Update, simulate_projectiles);

        // Flying straight along +X; the weave should push it off the X axis.
        let shot = app
            .world_mut()
            .spawn((
                Projectile,
                NetPos(Vec2::ZERO),
                Height(1000.0), // high up, so gravity won't crash it during the test
                ProjectileVelocity {
                    horizontal: Vec2::new(PROJECTILE_SPEED, 0.0),
                    vertical: 0.0,
                },
                ZigzagMotion { elapsed: 0.0 },
            ))
            .id();

        for _ in 0..15 {
            app.update();
        }
        let pos = app
            .world()
            .get::<NetPos>(shot)
            .expect("the zig-zag shot should still be alive")
            .0;
        assert!(
            pos.y.abs() > 1.0,
            "a zig-zag shot should deviate sideways (got y={})",
            pos.y
        );
        assert!(
            pos.x > 0.0,
            "a zig-zag shot should still travel forward (got x={})",
            pos.x
        );
    }
}
