//! Shared networking protocol: the components that replicate from server to
//! clients and the messages clients send back.
//!
//! This module compiles into both the client and the server binary so they
//! agree on the wire format. `bevy_replicon` hashes the registered protocol and
//! refuses connections whose hash differs, so a client built against a different
//! protocol is rejected automatically.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Authoritative world-space position of a dynamic entity (player or enemy).
///
/// This is the single source of truth for position in *all* modes: the server
/// (and offline single-player) write it from simulation, online clients receive
/// it via replication, and the client renderer copies it into [`Transform`].
/// Keeping position in its own small component means the server never needs a
/// `Transform`/renderer and the replicated payload stays tiny.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct NetPos(pub Vec2);

/// Movement request sent from a client to the server every frame.
///
/// `dir` is the (already normalized) desired movement direction, or zero when
/// the player is standing still. Sent unreliably: we transmit it every frame,
/// so a dropped packet simply self-heals on the next one.
#[derive(Event, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct PlayerInput {
    pub dir: Vec2,
}

/// Fire request sent from a client to the server when the player presses shoot.
///
/// Carries no aim data: the server fires in the player's tracked `Facing`, so the
/// client only needs to say "I shot". Sent on a reliable channel since a shot is
/// a discrete action we don't want to drop.
#[derive(Event, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct ShootRequest;
