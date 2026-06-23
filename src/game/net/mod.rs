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

pub mod input;
pub mod policy;
pub mod protocol;
pub mod spawn;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;

pub use input::{
    InputBackend, LatestLocalInput, LocalPlayerInput, is_input_backend_offline,
    is_input_backend_online,
};
pub use policy::{
    ClientEvent, NetworkAppExt, NetworkEvent, NetworkRegistered, Replicated, ReplicatedComponent,
    ServerEvent, apply_network_registry, init_network_registry,
};
pub use protocol::{
    ControllingClient, LastProcessedInput, MatchInfo, MatchPhase, NetPos, Owner, PlayerInput,
    ShieldRequest, ShootRequest, StartMatch, Winner, YouAreOwner,
};
pub use spawn::{SpawnCommandsExt, SpawnContext, resolve_spawn_context};

use bevy::prelude::*;

use super::projectile::Projectile;

/// Default UDP port the server listens on and clients connect to.
pub const DEFAULT_PORT: u16 = 5000;

/// Base protocol id identifying this game/version on the wire. Renet rejects
/// connections whose protocol id differs, giving a cheap version check on top of
/// Replicon's protocol-hash check. The active protocol id is this value XORed
/// with a hash of the join code (see [`protocol_id_for`]), so clients without the
/// server's code compute a different id and are refused at the netcode handshake.
pub const BASE_PROTOCOL_ID: u64 = 0x5342_525f_0001; // "SBR" + version

/// Deterministic FNV-1a hash of `bytes`. Unlike `std`'s `DefaultHasher`, this is
/// guaranteed to produce identical results in the client and server binaries (and
/// across runs), which the join-code gate relies on.
const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

/// The netcode protocol id for a given join `code`. An empty code yields the bare
/// [`BASE_PROTOCOL_ID`] (an open server); any non-empty code mixes its hash in, so
/// only peers that share the exact code agree on the id and can connect.
pub fn protocol_id_for(code: &str) -> u64 {
    if code.is_empty() {
        BASE_PROTOCOL_ID
    } else {
        BASE_PROTOCOL_ID ^ fnv1a(code.as_bytes())
    }
}

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

/// True only on the headless dedicated server. Used to position client-owned
/// players at match start (offline positions its single player separately).
pub fn is_server(role: Res<NetRole>) -> bool {
    *role == NetRole::Server
}

#[cfg(test)]
mod tests {
    use super::{BASE_PROTOCOL_ID, protocol_id_for};

    #[test]
    fn empty_code_yields_the_base_protocol_id() {
        assert_eq!(protocol_id_for(""), BASE_PROTOCOL_ID);
    }

    #[test]
    fn the_same_code_always_yields_the_same_id() {
        assert_eq!(protocol_id_for("secret"), protocol_id_for("secret"));
    }

    #[test]
    fn different_codes_yield_different_ids() {
        assert_ne!(protocol_id_for("secret"), protocol_id_for("other"));
        // A real code must differ from the open-server id too.
        assert_ne!(protocol_id_for("secret"), BASE_PROTOCOL_ID);
    }
}

/// Client-only marker on the player entity this client controls (see
/// [`client`](crate::game::net::client)). Its movement is predicted locally and
/// reconciled against the server, so it renders from [`PredictedPos`] rather than
/// the replicated [`NetPos`].
#[cfg(feature = "client")]
#[derive(Component)]
pub struct Predicted;

/// Client-only predicted position of the local player, advanced from local input
/// each fixed tick and reconciled against the confirmed [`NetPos`]. Rendered in
/// place of `NetPos` for the [`Predicted`] entity.
#[cfg(feature = "client")]
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PredictedPos(pub Vec2);

/// Copies the authoritative [`NetPos`] into the render [`Transform`] for every
/// dynamic entity. The local [`Predicted`] player instead renders from its
/// [`PredictedPos`] (so it reacts to input with no round-trip), smoothed snappily
/// to absorb reconciliation corrections. Offline positions snap; remote online
/// positions arrive at the tick rate and are interpolated. The `z` set when the
/// sprite was attached is preserved.
#[cfg(feature = "client")]
pub fn sync_netpos_to_transform(
    role: Res<NetRole>,
    time: Res<Time>,
    // Projectiles carry an altitude and are positioned by `render_projectiles`.
    mut query: Query<(&NetPos, Option<&PredictedPos>, &mut Transform), Without<Projectile>>,
) {
    let online = *role == NetRole::OnlineClient;
    for (pos, predicted, mut transform) in &mut query {
        // The controlled player follows its predicted position (snappy); every
        // other entity follows the replicated position (lerped online, snapped
        // offline).
        let (target, factor) = match predicted {
            Some(predicted) => (predicted.0, (30.0 * time.delta_secs()).min(1.0)),
            None if online => (pos.0, (15.0 * time.delta_secs()).min(1.0)),
            None => (pos.0, 1.0),
        };
        let smoothed = transform.translation.truncate().lerp(target, factor);
        transform.translation.x = smoothed.x;
        transform.translation.y = smoothed.y;
    }
}
