//! Shield: timed parry / block defensive ability.
//!
//! Holding the shield button raises a bubble for up to 2 seconds. While raised,
//! the player or bot is rooted, cannot shoot, and is invulnerable. For the first
//! 0.3 seconds after raising it, an incoming shot is **reflected** back along its
//! incoming path (with the reflector as the new owner). After that window the
//! shield still **destroys** shots, but no longer reflects them.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "client")]
use bevy::asset::RenderAssetUsages;
#[cfg(feature = "client")]
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::combat::{Dead, SpawnInvulnerability};
use super::net::{NetPos, is_authoritative};
use super::player::PLAYER_SIZE;
#[cfg(feature = "client")]
use super::player::{Player, PlayerColor};
use super::projectile::{PROJECTILE_RADIUS, ProjectileOwner, ProjectileVelocity};
use super::state::GameState;

#[cfg(feature = "client")]
use super::bot::Bot;

/// Max time the shield can stay raised.
const MAX_ACTIVE_DURATION: f32 = 2.0;
/// Minimum time the shield must stay raised once activated.
const MIN_ACTIVE_DURATION: f32 = 0.25;
/// Window after raising during which an impact reflects the shot.
const PARRY_WINDOW: f32 = 0.3;
/// Time after the shield drops before it is fully recharged.
const COOLDOWN_DURATION: f32 = 3.0;
/// How far outside the actor's hit radius a reflected shot is placed.
const REFLECT_PUSH: f32 = 2.0;

/// Replicated marker: the entity currently has its shield raised.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Shielding;

/// Replicated shield charge (0.0 = empty, 1.0 = full). Used by client visuals.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct ShieldCharge(pub f32);

/// Server/sim-only state machine for a shield.
#[derive(Component, Debug, Clone, Copy)]
pub struct ShieldState {
    pub status: ShieldStatus,
    /// Charge from 0.0 to 1.0. Drains while active, recharges on cooldown.
    pub charge: f32,
    /// Desired state from input/network. Picked up by [`tick_shields`].
    pub requested: bool,
}

impl Default for ShieldState {
    fn default() -> Self {
        Self {
            status: ShieldStatus::Ready,
            charge: 1.0,
            requested: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ShieldStatus {
    /// Shield is fully charged and ready to raise.
    Ready,
    /// Shield is raised. `raised_at` is `Time::elapsed_secs()` when it started.
    Active { raised_at: f32 },
    /// Shield is recharging after being lowered.
    Cooldown { elapsed: f32 },
}

/// System set for shield state ticking. Movement, shooting, and combat run after
/// this so they see the current `Shielding` marker.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShieldTickSet;

/// Marker for the visual bubble spawned as a child while shielding.
#[cfg(feature = "client")]
#[derive(Component)]
struct ShieldBubble;

/// Marker for the thin charge ring spawned as a child on every actor.
#[cfg(feature = "client")]
#[derive(Component)]
struct ShieldChargeRing;

/// Marker for the circular spawn-invulnerability timer ring spawned as a child.
#[cfg(feature = "client")]
#[derive(Component)]
struct InvulnTimerRing;

/// Marker that an actor already has shield visual children.
#[cfg(feature = "client")]
#[derive(Component)]
struct HasShieldVisuals;

/// Procedurally generated textures for the shield bubble and charge ring.
#[cfg(feature = "client")]
#[derive(Resource)]
struct ShieldTextures {
    bubble: Handle<Image>,
    ring: Handle<Image>,
}

pub struct ShieldPlugin;

impl Plugin for ShieldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            tick_shields
                .in_set(ShieldTickSet)
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        );

        #[cfg(feature = "client")]
        {
            app.add_systems(Startup, init_shield_textures).add_systems(
                Update,
                (
                    attach_shield_visuals.run_if(in_state(GameState::Playing)),
                    update_shield_visuals.run_if(in_state(GameState::Playing)),
                ),
            );
        }
    }
}

/// Inserts the server-side shield state and replicated charge on a newly spawned
/// player or bot.
pub fn insert_shield(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .insert((ShieldState::default(), ShieldCharge(1.0)));
}

/// Updates `ShieldState` and adds/removes the replicated `Shielding` marker.
fn tick_shields(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut ShieldState, Option<&Shielding>), Without<Dead>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();

    for (entity, mut state, _is_active) in &mut query {
        match state.status {
            ShieldStatus::Ready => {
                if state.requested && state.charge > 0.0 {
                    state.status = ShieldStatus::Active { raised_at: now };
                    state.charge = state.charge.min(1.0);
                    commands.entity(entity).insert(Shielding);
                } else {
                    // Slowly top up in case we ever allow partial ready states.
                    state.charge = (state.charge + dt / COOLDOWN_DURATION).min(1.0);
                }
            }
            ShieldStatus::Active { raised_at } => {
                let elapsed = now - raised_at;
                state.charge -= dt / MAX_ACTIVE_DURATION;

                let min_done = elapsed >= MIN_ACTIVE_DURATION;
                let depleted = state.charge <= 0.0;
                let expired = elapsed >= MAX_ACTIVE_DURATION;

                if depleted || expired || (!state.requested && min_done) {
                    state.status = ShieldStatus::Cooldown { elapsed: 0.0 };
                    commands.entity(entity).remove::<Shielding>();
                }
            }
            ShieldStatus::Cooldown { elapsed } => {
                let new_elapsed = elapsed + dt;
                state.charge = (new_elapsed / COOLDOWN_DURATION).min(1.0);

                if state.charge >= 1.0 {
                    state.status = ShieldStatus::Ready;
                } else {
                    state.status = ShieldStatus::Cooldown {
                        elapsed: new_elapsed,
                    };
                }

                // If the player is already requesting shield again, it will raise
                // as soon as Ready is reached (on a later frame).
            }
        }

        // Keep the replicated charge in sync so the UI is accurate.
        commands.entity(entity).insert(ShieldCharge(state.charge));
    }
}

/// Returns true if the given shield is currently in its parry window.
pub fn is_parry_window(shield: &ShieldState, time: &Time) -> bool {
    match shield.status {
        ShieldStatus::Active { raised_at } => time.elapsed_secs() - raised_at <= PARRY_WINDOW,
        _ => false,
    }
}

/// Reflects a projectile off a shield. Reverses velocity, moves the projectile
/// outside the actor, and transfers ownership to the reflector.
pub fn reflect_projectile(
    projectile_pos: &mut NetPos,
    velocity: &mut ProjectileVelocity,
    owner: &mut ProjectileOwner,
    reflector: Entity,
    reflector_pos: Vec2,
) {
    let away = (projectile_pos.0 - reflector_pos).normalize_or_zero();
    projectile_pos.0 =
        reflector_pos + away * (PLAYER_SIZE / 2.0 + PROJECTILE_RADIUS + REFLECT_PUSH);
    velocity.horizontal = -velocity.horizontal;
    owner.0 = reflector;
}

#[cfg(feature = "client")]
fn init_shield_textures(mut commands: Commands, images: Option<ResMut<Assets<Image>>>) {
    let Some(mut images) = images else {
        return;
    };
    let bubble = images.add(make_radial(64, |r| {
        // Soft filled bubble, stronger near the edge.
        let d = (r - 0.82) / 0.15;
        ((-d * d).exp() * 0.7 + (1.0 - r).clamp(0.0, 1.0) * 0.3).clamp(0.0, 1.0)
    }));
    let ring = images.add(make_radial(64, |r| {
        // Thin ring near the outside.
        let d = (r - 0.85) / 0.08;
        (-d * d).exp()
    }));
    commands.insert_resource(ShieldTextures { bubble, ring });
}

#[cfg(feature = "client")]
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

#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn attach_shield_visuals(
    mut commands: Commands,
    textures: Option<Res<ShieldTextures>>,
    actors: Query<Entity, (Or<(With<Player>, With<Bot>)>, Without<HasShieldVisuals>)>,
) {
    let Some(textures) = textures else {
        return;
    };
    for entity in &actors {
        let bubble = commands
            .spawn((
                ShieldBubble,
                Sprite {
                    image: textures.bubble.clone(),
                    color: Color::srgba(1.0, 1.0, 1.0, 0.0),
                    custom_size: Some(Vec2::splat(PLAYER_SIZE * 1.6)),
                    ..default()
                },
                Transform::from_xyz(0.0, 0.0, 0.2),
                Visibility::Hidden,
                super::InGame,
            ))
            .id();

        let ring = commands
            .spawn((
                ShieldChargeRing,
                Sprite {
                    image: textures.ring.clone(),
                    color: Color::srgba(1.0, 1.0, 1.0, 0.0),
                    custom_size: Some(Vec2::splat(PLAYER_SIZE * 1.8)),
                    ..default()
                },
                Transform::from_xyz(0.0, 0.0, 0.3),
                Visibility::Visible,
                super::InGame,
            ))
            .id();

        let invuln = commands
            .spawn((
                InvulnTimerRing,
                Sprite {
                    image: textures.ring.clone(),
                    color: Color::srgba(0.85, 0.85, 0.85, 0.0),
                    custom_size: Some(Vec2::splat(PLAYER_SIZE * 1.85)),
                    ..default()
                },
                Transform::from_xyz(0.0, 0.0, 0.35),
                Visibility::Hidden,
                super::InGame,
            ))
            .id();

        commands
            .entity(entity)
            .add_children(&[bubble, ring, invuln])
            .insert(HasShieldVisuals);
    }
}

#[cfg(feature = "client")]
#[allow(clippy::type_complexity)]
fn update_shield_visuals(
    actors: Query<
        (
            &ShieldCharge,
            Option<&Shielding>,
            Option<&SpawnInvulnerability>,
            &PlayerColor,
            &Children,
            Has<Dead>,
        ),
        Or<(With<Player>, With<Bot>)>,
    >,
    mut bubbles: Query<
        (&mut Visibility, &mut Sprite),
        (
            With<ShieldBubble>,
            Without<ShieldChargeRing>,
            Without<InvulnTimerRing>,
        ),
    >,
    mut rings: Query<
        (&mut Visibility, &mut Sprite),
        (
            With<ShieldChargeRing>,
            Without<ShieldBubble>,
            Without<InvulnTimerRing>,
        ),
    >,
    mut invulns: Query<
        (&mut Visibility, &mut Sprite),
        (
            With<InvulnTimerRing>,
            Without<ShieldBubble>,
            Without<ShieldChargeRing>,
        ),
    >,
) {
    for (charge, shielding, invuln, color, children, dead) in &actors {
        let glow = player_glow(*color);

        for child in children {
            if let Ok((mut visibility, mut sprite)) = bubbles.get_mut(*child) {
                if !dead && invuln.is_none() && shielding.is_some() {
                    *visibility = Visibility::Visible;
                    sprite.color = glow.with_alpha(0.55);
                } else {
                    *visibility = Visibility::Hidden;
                }
            }

            if let Ok((mut visibility, mut sprite)) = rings.get_mut(*child) {
                if dead || invuln.is_some() {
                    // Dead actors shouldn't show any shield indicator; invulnerability
                    // uses its own timer ring instead.
                    *visibility = Visibility::Hidden;
                    sprite.color = glow.with_alpha(0.0);
                } else if charge.0 >= 1.0 {
                    *visibility = Visibility::Visible;
                    // Show a subtle readiness ring when fully charged.
                    sprite.color = glow.with_alpha(0.25);
                } else {
                    *visibility = Visibility::Visible;
                    // Fade out while recharging so the player knows it is not ready.
                    sprite.color = glow.with_alpha(0.08 * charge.0);
                }
            }

            if let Ok((mut visibility, mut sprite)) = invulns.get_mut(*child) {
                if !dead && let Some(inv) = invuln {
                    *visibility = Visibility::Visible;
                    let t = (inv.remaining / inv.max).clamp(0.0, 1.0);
                    // The ring fills (becomes more opaque) as invulnerability runs out.
                    sprite.color = Color::srgba(0.85, 0.85, 0.85, 1.0 - t);
                } else {
                    *visibility = Visibility::Hidden;
                }
            }
        }
    }
}

/// A soft glow version of a player color for shield visuals.
#[cfg(feature = "client")]
fn player_glow(color: PlayerColor) -> Color {
    use super::projectile::shot_glow;
    shot_glow(color).with_alpha(1.0)
}
