//! Unified local input sampling and backend routing.
//!
//! The client binary (offline or online) samples the keyboard into a single
//! [`LocalPlayerInput`] event each frame. From there the active backend routes it:
//!
//! - Offline: applies directly to `PlayerIntent`, `ShieldState`, and fires shots.
//! - Online client: converts to network events (`PlayerInput`, `ShootRequest`,
//!   `ShieldRequest`).
//!
//! This keeps gameplay modules (`player`, `projectile`, `shield`) free of
//! role-specific input branching.

use bevy::prelude::*;

use crate::game::net::NetRole;

/// Local input sample produced every frame by the windowed client. The backend
/// (offline or online client) decides how to act on it.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalPlayerInput {
    /// Normalized movement direction, or zero when standing still.
    pub dir: Vec2,
    /// True on the frame the player pressed the shoot button.
    pub shoot: bool,
    /// `Some(pressed)` when the shield button changed state this frame; `None`
    /// when it stayed the same.
    pub shield_change: Option<bool>,
}

/// Most recent local input sample. Updated by [`sample_local_input`] so fixed-step
/// systems (movement prediction) can read it without resampling the keyboard.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct LatestLocalInput(pub LocalPlayerInput);

/// Samples keyboard input and stores the latest sample in [`LatestLocalInput`].
/// Client-only: the server has no local keyboard.
#[cfg(feature = "client")]
pub fn sample_local_input(
    input: Res<ButtonInput<KeyCode>>,
    mut last_shield: Local<bool>,
    mut latest: ResMut<LatestLocalInput>,
) {
    let mut dir = Vec2::ZERO;
    if input.pressed(KeyCode::KeyW) || input.pressed(KeyCode::ArrowUp) {
        dir.y += 1.0;
    }
    if input.pressed(KeyCode::KeyS) || input.pressed(KeyCode::ArrowDown) {
        dir.y -= 1.0;
    }
    if input.pressed(KeyCode::KeyA) || input.pressed(KeyCode::ArrowLeft) {
        dir.x -= 1.0;
    }
    if input.pressed(KeyCode::KeyD) || input.pressed(KeyCode::ArrowRight) {
        dir.x += 1.0;
    }
    if dir != Vec2::ZERO {
        dir = dir.normalize();
    }

    let shoot = input.just_pressed(KeyCode::Space);

    let shield_pressed = input.pressed(KeyCode::ShiftLeft) || input.pressed(KeyCode::ShiftRight);
    let shield_change = if shield_pressed != *last_shield {
        *last_shield = shield_pressed;
        Some(shield_pressed)
    } else {
        None
    };

    latest.0 = LocalPlayerInput {
        dir,
        shoot,
        shield_change,
    };
}

/// Resource inserted by the binary that determines how local input is routed.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputBackend {
    /// Apply input locally (offline single-player).
    #[default]
    Offline,
    /// Send input to the remote server (online client).
    Online,
}

impl InputBackend {
    /// Choose the backend from the active [`NetRole`].
    pub fn from_role(role: NetRole) -> Self {
        match role {
            NetRole::Offline => InputBackend::Offline,
            NetRole::OnlineClient => InputBackend::Online,
            NetRole::Server => InputBackend::Offline, // server has no local input
        }
    }
}

/// Run condition: local input should be applied locally (offline single-player).
pub fn is_input_backend_offline(backend: Res<InputBackend>) -> bool {
    *backend == InputBackend::Offline
}

/// Run condition: local input should be sent to the server (online client).
pub fn is_input_backend_online(backend: Res<InputBackend>) -> bool {
    *backend == InputBackend::Online
}
