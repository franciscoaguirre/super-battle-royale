//! Shared networking protocol: the components that replicate from server to
//! clients and the messages clients send back.
//!
//! This module compiles into both the client and the server binary so they
//! agree on the wire format. `bevy_replicon` hashes the registered protocol and
//! refuses connections whose hash differs, so a client built against a different
//! protocol is rejected automatically.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::game::player::PlayerColor;

/// Authoritative world-space position of a dynamic entity (player or bot).
///
/// This is the single source of truth for position in *all* modes: the server
/// (and offline single-player) write it from simulation, online clients receive
/// it via replication, and the client renderer copies it into [`Transform`].
/// Keeping position in its own small component means the server never needs a
/// `Transform`/renderer and the replicated payload stays tiny.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct NetPos(pub Vec2);

/// Movement request sent from a client to the server once per fixed tick.
///
/// `dir` is the (already normalized) desired movement direction, or zero when
/// the player is standing still. `seq` is a per-client monotonic input counter
/// (one per fixed tick) used by client-side prediction: the server echoes the
/// last applied `seq` back via [`LastProcessedInput`] so the client knows which
/// of its buffered inputs are acknowledged and which to replay. Sent unreliably:
/// a dropped packet self-heals on the next tick (reconciliation absorbs the gap).
#[derive(Event, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct PlayerInput {
    pub dir: Vec2,
    pub seq: u32,
}

/// Replicated onto each player: the `seq` of the most recent [`PlayerInput`] the
/// server has applied to that player. The controlling client reads this on its
/// own player to discard acknowledged inputs and replay the rest during
/// reconciliation. Authoritative-written, replicated to all clients.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct LastProcessedInput(pub u32);

/// Replicated onto each player: the messaging-backend id (renet `NetworkId`) of
/// the client that controls it. A client identifies *its own* player by matching
/// this against its local `NetcodeClientTransport::client_id()`, then predicts
/// only that entity. Replicated (rides the replication stream, so unlike a
/// directed event it can't lose the spawn race).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct ControllingClient(pub u64);

/// Fire request sent from a client to the server when the player presses shoot.
///
/// Carries no aim data: the server fires in the player's tracked `Facing`, so the
/// client only needs to say "I shot". Sent on a reliable channel since a shot is
/// a discrete action we don't want to drop.
#[derive(Event, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct ShootRequest;

/// Shield request sent from a client to the server whenever the shield button
/// is pressed or released.
///
/// `active` is the desired state: true while the button is held, false on
/// release. Sent reliably because it is a discrete state change.
#[derive(Event, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct ShieldRequest {
    pub active: bool,
}

/// Marks the player entity belonging to the game's owner: the first client to
/// join (or the local player when offline). Replicated so the server stays the
/// authority on who may start the match; clients learn they are the owner through
/// the [`YouAreOwner`] event instead, since Replicon does not tag a client's own
/// entity.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Owner;

/// Replicated singleton owned by the authoritative side: the live match state.
/// It is the cross-network "what should I be doing" signal — online clients
/// mirror their local [`GameState`](crate::game::state::GameState) from it (load
/// `map_index`, show the winner, advance maps). Spawned once when the first match
/// starts and mutated thereafter (never re-spawned). Must not carry `InGame`, or
/// it would be despawned on a state-exit cleanup.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct MatchInfo {
    /// Which map (index into [`MAPS`](crate::game::map::MAPS)) the current round uses.
    pub map_index: u8,
    /// Increments each time a new level starts, so clients detect "switch map".
    pub round: u32,
    /// Whether the round is live or has ended (winner announced).
    pub phase: MatchPhase,
    /// Who won the last round; meaningful while `phase == Ended`.
    pub winner: Winner,
}

/// The live/ended state of the current round (a field of [`MatchInfo`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchPhase {
    /// The round is being played.
    #[default]
    Playing,
    /// The round is over and the winner is being announced.
    Ended,
}

/// The outcome of a round (a field of [`MatchInfo`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Winner {
    /// Nobody survived (the last combatants died together).
    #[default]
    Draw,
    /// A human player survived, identified by their color.
    Player(PlayerColor),
    /// A bot survived.
    Bot,
}

/// Sent by the owner's client to ask the server to start the match with the
/// chosen map and bot count. The server validates that the sender owns [`Owner`].
#[derive(Event, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct StartMatch {
    pub map_index: u8,
    pub bot_count: u8,
}

/// Sent by the server to a single client right after it joins as the owner, so
/// the client knows to show the lobby's configuration controls and Start button.
#[derive(Event, Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct YouAreOwner;
