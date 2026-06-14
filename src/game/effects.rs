//! Client-side combat "juice": impact sparks, expanding shockwave rings, screen
//! shake, and a chromatic-aberration pulse on hits and deaths.
//!
//! Everything is driven by the same replicated signals the audio uses — `Impact`
//! markers (which now carry their world position) and the `Dead` marker — so the
//! effects fire identically offline, on the host, and on every connected client.
//! The shockwave ring and spark use small textures generated at startup, so no
//! art assets are needed.

use bevy::asset::RenderAssetUsages;
use bevy::post_process::effect_stack::ChromaticAberration;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::InGame;
use super::combat::Dead;
use super::net::NetPos;
use super::player::PlayerColor;
use super::projectile::{Impact, ImpactKind, shot_glow};
use super::state::GameState;

const SPARK_SIZE: f32 = 8.0;
const SPARK_LIFETIME: f32 = 0.4;
const SPARK_DRAG: f32 = 4.0;

const SHOCKWAVE_LIFETIME: f32 = 0.35;
const SHOCKWAVE_START_SIZE: f32 = 12.0;
const SHOCKWAVE_END_SIZE: f32 = 130.0;

const SHAKE_MAX_OFFSET: f32 = 16.0;
const TRAUMA_DECAY: f32 = 2.0;
const ABERRATION_MAX: f32 = 0.06;
const ABERRATION_DECAY: f32 = 0.4;

// Impact glow colors (linear HDR so they bloom).
const GROUND_GLOW: Color = Color::linear_rgb(4.0, 2.6, 1.0);
const OBJECT_GLOW: Color = Color::linear_rgb(8.0, 8.0, 8.0);

/// Screen-feedback accumulators, decayed every frame and applied to the camera.
#[derive(Resource, Default)]
struct Feedback {
    trauma: f32,
    aberration: f32,
}

/// Handles to the procedurally-generated effect textures.
#[derive(Resource)]
struct FxTextures {
    dot: Handle<Image>,
    ring: Handle<Image>,
}

/// A flying, fading spark particle.
#[derive(Component)]
struct Spark {
    velocity: Vec2,
    timer: Timer,
}

/// An expanding, fading shockwave ring.
#[derive(Component)]
struct Shockwave {
    timer: Timer,
    color: Color,
}

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Feedback>()
            .add_systems(Startup, init_fx_textures)
            .add_systems(
                Update,
                (
                    spawn_impact_effects,
                    spawn_death_effects,
                    update_sparks,
                    update_shockwaves,
                    apply_feedback,
                )
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

/// Builds a soft round dot and a soft ring texture once at startup.
fn init_fx_textures(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let dot = images.add(make_radial(32, |r| (1.0 - r).clamp(0.0, 1.0).powf(1.5)));
    let ring = images.add(make_radial(64, |r| {
        // A soft band centred near the edge, giving a ring.
        let d = (r - 0.78) / 0.13;
        (-d * d).exp()
    }));
    commands.insert_resource(FxTextures { dot, ring });
}

/// Creates a `size`x`size` white RGBA texture whose alpha is `profile(radius)`,
/// with radius 0 at the centre and 1 at the edge.
fn make_radial(size: usize, profile: impl Fn(f32) -> f32) -> Image {
    let mut data = vec![0u8; size * size * 4];
    let half = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let nx = (x as f32 + 0.5 - half) / half;
            let ny = (y as f32 + 0.5 - half) / half;
            let r = (nx * nx + ny * ny).sqrt();
            let alpha = (profile(r) * 255.0).clamp(0.0, 255.0) as u8;
            let i = (y * size + x) * 4;
            data[i] = 255;
            data[i + 1] = 255;
            data[i + 2] = 255;
            data[i + 3] = alpha;
        }
    }
    Image::new(
        Extent3d {
            width: size as u32,
            height: size as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// Spawns sparks + a shockwave (and shakes the screen) wherever a shot ended.
#[allow(clippy::type_complexity)]
fn spawn_impact_effects(
    mut commands: Commands,
    textures: Res<FxTextures>,
    mut feedback: ResMut<Feedback>,
    impacts: Query<(&Impact, &NetPos), Added<Impact>>,
) {
    for (impact, pos) in &impacts {
        match impact.0 {
            ImpactKind::Ground => {
                spawn_sparks(&mut commands, &textures.dot, pos.0, GROUND_GLOW, 5, 90.0);
                spawn_shockwave(&mut commands, &textures.ring, pos.0, GROUND_GLOW);
                feedback.trauma = (feedback.trauma + 0.18).min(1.0);
            }
            ImpactKind::Object => {
                spawn_sparks(&mut commands, &textures.dot, pos.0, OBJECT_GLOW, 12, 150.0);
                spawn_shockwave(&mut commands, &textures.ring, pos.0, OBJECT_GLOW);
                feedback.trauma = (feedback.trauma + 0.4).min(1.0);
                feedback.aberration = (feedback.aberration + 0.04).min(ABERRATION_MAX);
            }
            ImpactKind::Shield | ImpactKind::Parry => {
                // Shield interactions get a smaller, whiter burst.
                spawn_sparks(&mut commands, &textures.dot, pos.0, OBJECT_GLOW, 6, 100.0);
                spawn_shockwave(&mut commands, &textures.ring, pos.0, OBJECT_GLOW);
                feedback.trauma = (feedback.trauma + 0.15).min(1.0);
            }
        }
    }
}

/// A bigger burst in the player's color when they die.
#[allow(clippy::type_complexity)]
fn spawn_death_effects(
    mut commands: Commands,
    textures: Res<FxTextures>,
    mut feedback: ResMut<Feedback>,
    dead: Query<(&NetPos, &PlayerColor), Added<Dead>>,
) {
    for (pos, color) in &dead {
        let glow = shot_glow(*color);
        spawn_sparks(&mut commands, &textures.dot, pos.0, glow, 20, 210.0);
        spawn_shockwave(&mut commands, &textures.ring, pos.0, glow);
        feedback.trauma = 1.0;
        feedback.aberration = ABERRATION_MAX;
    }
}

fn spawn_sparks(
    commands: &mut Commands,
    texture: &Handle<Image>,
    origin: Vec2,
    color: Color,
    count: usize,
    speed: f32,
) {
    // Deterministic spread, offset by position so each impact looks different.
    let seed = origin.x * 0.7 + origin.y * 1.3;
    for i in 0..count {
        let angle = (i as f32 / count as f32) * std::f32::consts::TAU + seed;
        let dir = Vec2::new(angle.cos(), angle.sin());
        let jitter = (i as f32 * 1.6 + seed).sin() * 0.5 + 0.5;
        commands.spawn((
            Spark {
                velocity: dir * speed * (0.6 + 0.5 * jitter),
                timer: Timer::from_seconds(SPARK_LIFETIME, TimerMode::Once),
            },
            Sprite {
                image: texture.clone(),
                color,
                custom_size: Some(Vec2::splat(SPARK_SIZE)),
                ..default()
            },
            // Above the playfield — sparks fly over everything.
            Transform::from_xyz(origin.x, origin.y, 21.0),
            InGame,
        ));
    }
}

fn spawn_shockwave(commands: &mut Commands, texture: &Handle<Image>, origin: Vec2, color: Color) {
    commands.spawn((
        Shockwave {
            timer: Timer::from_seconds(SHOCKWAVE_LIFETIME, TimerMode::Once),
            color,
        },
        Sprite {
            image: texture.clone(),
            color,
            custom_size: Some(Vec2::splat(SHOCKWAVE_START_SIZE)),
            ..default()
        },
        // On the ground (above floor/walls, beneath players).
        Transform::from_xyz(origin.x, origin.y, 2.0),
        InGame,
    ));
}

fn update_sparks(
    time: Res<Time>,
    mut commands: Commands,
    mut sparks: Query<(Entity, &mut Spark, &mut Transform, &mut Sprite)>,
) {
    let dt = time.delta_secs();
    for (entity, mut spark, mut transform, mut sprite) in &mut sparks {
        if spark.timer.tick(time.delta()).just_finished() {
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation.x += spark.velocity.x * dt;
        transform.translation.y += spark.velocity.y * dt;
        spark.velocity *= 1.0 - (SPARK_DRAG * dt).min(1.0);
        let remaining = 1.0 - spark.timer.fraction();
        sprite.color = sprite.color.with_alpha(remaining);
    }
}

fn update_shockwaves(
    time: Res<Time>,
    mut commands: Commands,
    mut shockwaves: Query<(Entity, &mut Shockwave, &mut Sprite)>,
) {
    for (entity, mut shockwave, mut sprite) in &mut shockwaves {
        if shockwave.timer.tick(time.delta()).just_finished() {
            commands.entity(entity).despawn();
            continue;
        }
        let progress = shockwave.timer.fraction();
        let size = SHOCKWAVE_START_SIZE + (SHOCKWAVE_END_SIZE - SHOCKWAVE_START_SIZE) * progress;
        sprite.custom_size = Some(Vec2::splat(size));
        sprite.color = shockwave.color.with_alpha(1.0 - progress);
    }
}

/// Applies the accumulated trauma (camera shake) and aberration to the camera,
/// then decays both toward zero.
fn apply_feedback(
    time: Res<Time>,
    mut feedback: ResMut<Feedback>,
    mut camera: Query<(&mut Transform, &mut ChromaticAberration), With<Camera>>,
) {
    let dt = time.delta_secs();
    if let Ok((mut transform, mut aberration)) = camera.single_mut() {
        let trauma = feedback.trauma.clamp(0.0, 1.0);
        let shake = trauma * trauma; // quadratic falloff feels punchier
        let t = time.elapsed_secs();
        transform.translation.x = (t * 53.0).sin() * shake * SHAKE_MAX_OFFSET;
        transform.translation.y = (t * 61.0).cos() * shake * SHAKE_MAX_OFFSET;
        aberration.intensity = feedback.aberration.min(ABERRATION_MAX);
    }
    feedback.trauma = (feedback.trauma - TRAUMA_DECAY * dt).max(0.0);
    feedback.aberration = (feedback.aberration - ABERRATION_DECAY * dt).max(0.0);
}
