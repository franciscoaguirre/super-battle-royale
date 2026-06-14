//! Multiplayer networking.
//!
//! The game runs in one of three [`NetRole`]s, chosen at startup by the binary
//! that launched it:
//!
//! - [`NetRole::Offline`] — the windowed client with no server: it simulates and
//!   renders locally, exactly like the original single-player game.
//! - [`NetRole::OnlineClient`] — the windowed client connected to a server: it
//!   renders and sends input, but runs no simulation (the server is authority).
//! - [`NetRole::Server`] — the headless dedicated server: it simulates and
//!   replicates, but never renders.
//!
//! Simulation systems are gated on [`is_authoritative`] (offline + server) and
//! rendering systems are compiled only into the client binary (the `client`
//! feature). The networking transport itself lives in [`client`]/[`server`],
//! which are added by their respective binaries.

pub mod protocol;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;

pub use protocol::{NetPos, PlayerInput, ShootRequest};

use bevy::prelude::*;
use bevy_replicon::prelude::*;

use super::combat::{Dead, Health};
use super::enemy::Enemy;
use super::player::{Player, PlayerColor};
use super::projectile::{Height, Impact, Projectile, ShotColor};

/// Default UDP port the server listens on and clients connect to.
pub const DEFAULT_PORT: u16 = 5000;

/// Identifies this game/version on the wire. Renet rejects connections whose
/// protocol id differs, giving a cheap version check on top of Replicon's
/// protocol-hash check.
pub const PROTOCOL_ID: u64 = 0x5342_525f_0001; // "SBR" + version

/// Which role this running instance plays. Inserted as a resource before the app
/// starts so run-conditions can branch on it.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum NetRole {
    /// Local single-player: simulate and render, no networking.
    Offline,
    /// Connected client: render and send input, no local simulation.
    OnlineClient,
    /// Headless dedicated server: simulate and replicate, no rendering.
    Server,
}

/// Registers the replicated components and client messages that make up the
/// protocol. Must be called *after* `RepliconPlugins` is added (it relies on the
/// replication registry), so it lives inside the client/server net plugins.
pub fn register_protocol(app: &mut App) {
    app.replicate::<NetPos>()
        .replicate::<Player>()
        .replicate::<PlayerColor>()
        .replicate::<Health>()
        .replicate::<Enemy>()
        .replicate::<Projectile>()
        .replicate::<Height>()
        .replicate::<ShotColor>()
        .replicate::<Impact>()
        .replicate::<Dead>()
        .add_client_event::<PlayerInput>(Channel::Unreliable)
        .add_client_event::<ShootRequest>(Channel::Ordered);
}

/// True when this instance owns the simulation: offline single-player or the
/// dedicated server. Online clients are *not* authoritative.
pub fn is_authoritative(role: Res<NetRole>) -> bool {
    matches!(*role, NetRole::Offline | NetRole::Server)
}

/// True only in offline single-player (drives local keyboard movement).
pub fn is_offline(role: Res<NetRole>) -> bool {
    *role == NetRole::Offline
}

/// True only when connected to a remote server (drives input sending).
pub fn is_online_client(role: Res<NetRole>) -> bool {
    *role == NetRole::OnlineClient
}

/// Copies the authoritative [`NetPos`] into the render [`Transform`] for every
/// dynamic entity. Offline positions are local and exact, so they snap; online
/// positions arrive at the server tick rate, so they are interpolated for
/// smoothness. The `z` set when the sprite was attached is preserved.
#[cfg(feature = "client")]
pub fn sync_netpos_to_transform(
    role: Res<NetRole>,
    time: Res<Time>,
    // Projectiles carry an altitude and are positioned by `render_projectiles`.
    mut query: Query<(&NetPos, &mut Transform), Without<Projectile>>,
) {
    let online = *role == NetRole::OnlineClient;
    for (pos, mut transform) in &mut query {
        let target = pos.0;
        if online {
            let factor = (15.0 * time.delta_secs()).min(1.0);
            let smoothed = transform.translation.truncate().lerp(target, factor);
            transform.translation.x = smoothed.x;
            transform.translation.y = smoothed.y;
        } else {
            transform.translation.x = target.x;
            transform.translation.y = target.y;
        }
    }
}
