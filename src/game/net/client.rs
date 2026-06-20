//! Client-side networking: connects to a server and forwards local input.
//!
//! Added only when the game is launched with a server address. Rendering of the
//! replicated world (sprites, position interpolation) is handled by the regular
//! gameplay plugins, gated on the `client` feature.

use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::SystemTime;

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::{
    RenetChannelsExt, RenetClient, RepliconRenetPlugins,
    netcode::{ClientAuthentication, NetcodeClientTransport},
    renet::ConnectionConfig,
};

use super::{
    ControllingClient, LastProcessedInput, NetPos, PlayerInput, Predicted, PredictedPos,
    ShieldRequest, ShootRequest, protocol_id_for, register_protocol,
};
use crate::game::combat::Dead;
use crate::game::map::{ArenaBounds, CurrentMap, TileMap};
use crate::game::player::{FIXED_DT, Player, input_direction, step_player};
use crate::game::state::GameState;

/// Largest number of un-acknowledged inputs the client keeps for replay before
/// dropping the oldest (a safety bound if acks stall; normally pruned far below).
const INPUT_HISTORY_CAP: usize = 256;

/// The server endpoint this client should connect to.
#[derive(Resource, Clone, Copy)]
struct ServerEndpoint(SocketAddr);

/// The netcode protocol id derived from the join code; must match the server's.
#[derive(Resource, Clone, Copy)]
struct ClientProtocolId(u64);

/// Monotonic counter stamped onto each [`PlayerInput`] (one per fixed tick), so
/// the server can ack the last applied input and the client can match its replay
/// buffer to that ack.
#[derive(Resource, Default)]
struct InputSeq(u32);

/// Ring buffer of recently-sent `(seq, dir)` inputs awaiting acknowledgement.
/// Reconciliation drops acked entries and replays the rest from the confirmed
/// server position to recompute the predicted position.
#[derive(Resource, Default)]
struct InputHistory(VecDeque<(u32, Vec2)>);

/// Connects the windowed game to a dedicated server.
pub struct ClientNetPlugin {
    pub server_addr: SocketAddr,
    /// Join code supplied by the player; gates connection via the protocol id.
    pub join_code: String,
}

impl Plugin for ClientNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RepliconPlugins, RepliconRenetPlugins));
        register_protocol(app);
        app.insert_resource(ServerEndpoint(self.server_addr))
            .insert_resource(ClientProtocolId(protocol_id_for(&self.join_code)))
            .init_resource::<InputSeq>()
            .init_resource::<InputHistory>()
            .add_systems(Startup, setup_client)
            // Identify our own player + fire requests run every frame.
            .add_systems(
                Update,
                (tag_local_player, send_shoot_request, send_shield_request)
                    .run_if(in_state(ClientState::Connected))
                    .run_if(in_state(GameState::Playing)),
            )
            // Prediction samples + sends input and reconciles on the fixed tick,
            // matching the server's fixed-step movement.
            .add_systems(
                FixedUpdate,
                predict_and_reconcile
                    .run_if(in_state(ClientState::Connected))
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

/// Creates the renet client + netcode transport from the configured endpoint.
fn setup_client(
    mut commands: Commands,
    channels: Res<RepliconChannels>,
    endpoint: Res<ServerEndpoint>,
    protocol: Res<ClientProtocolId>,
) -> Result<()> {
    let client = RenetClient::new(ConnectionConfig {
        server_channels_config: channels.server_configs(),
        client_channels_config: channels.client_configs(),
        ..Default::default()
    });

    let current_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
    // A throwaway per-session id; the server identifies players by entity, so the
    // exact value only needs to be unique among concurrent connections.
    let client_id = current_time.as_millis() as u64;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let authentication = ClientAuthentication::Unsecure {
        client_id,
        protocol_id: protocol.0,
        server_addr: endpoint.0,
        user_data: None,
    };
    let transport = NetcodeClientTransport::new(current_time, authentication, socket)?;

    commands.insert_resource(client);
    commands.insert_resource(transport);
    info!("connecting to server at {}", endpoint.0);

    Ok(())
}

/// Finds the player entity this client controls — the replicated one whose
/// [`ControllingClient`] matches our own netcode id — and marks it [`Predicted`]
/// so it's simulated locally and reconciled. Idempotent (re-tags on respawn /
/// reconnect); rides replication, so it can't lose the spawn race.
#[allow(clippy::type_complexity)]
fn tag_local_player(
    mut commands: Commands,
    transport: Option<Res<NetcodeClientTransport>>,
    players: Query<(Entity, &ControllingClient, &NetPos), (With<Player>, Without<Predicted>)>,
) {
    let Some(transport) = transport else {
        return;
    };
    let my_id = transport.client_id();
    for (entity, controller, pos) in &players {
        if controller.0 == my_id {
            commands
                .entity(entity)
                .insert((Predicted, PredictedPos(pos.0)));
        }
    }
}

/// Per fixed tick: sample local input, buffer + send it (tagged with a seq), then
/// reconcile the local player — anchor to the server-confirmed [`NetPos`], drop
/// acknowledged inputs, and replay the rest through the shared [`step_player`] to
/// recompute [`PredictedPos`]. The predicted position is what the local player
/// renders from, so movement responds with no round-trip.
#[allow(clippy::type_complexity)]
fn predict_and_reconcile(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut seq: ResMut<InputSeq>,
    mut history: ResMut<InputHistory>,
    map: Res<CurrentMap>,
    bounds: Res<ArenaBounds>,
    mut player: Query<
        (&NetPos, &mut PredictedPos, &LastProcessedInput, Has<Dead>),
        With<Predicted>,
    >,
) {
    // Always sample, buffer, and send this tick's input — even before our player
    // is tagged — so the server starts moving us immediately.
    let dir = input_direction(&keys);
    seq.0 = seq.0.wrapping_add(1);
    let this_seq = seq.0;
    history.0.push_back((this_seq, dir));
    if history.0.len() > INPUT_HISTORY_CAP {
        history.0.pop_front();
    }
    commands.client_trigger(PlayerInput { dir, seq: this_seq });

    let Ok((net_pos, mut predicted, ack, dead)) = player.single_mut() else {
        return; // our player isn't tagged yet
    };

    // Dead players don't move: snap to the confirmed position and forget history.
    if dead {
        predicted.0 = net_pos.0;
        history.0.clear();
        return;
    }

    // Reconcile: anchor to the confirmed position and replay un-acked inputs.
    predicted.0 = reconcile_pos(net_pos.0, ack.0, &mut history.0, &map.0, &bounds);
}

/// Recomputes a predicted position: drop inputs from `history` whose seq is `<=`
/// the server-acknowledged `ack`, then replay the remaining inputs from the
/// server-`confirmed` position through [`step_player`]. Pure (the only mutation
/// is pruning `history`), so it's unit-tested directly.
fn reconcile_pos(
    confirmed: Vec2,
    ack: u32,
    history: &mut VecDeque<(u32, Vec2)>,
    map: &TileMap,
    bounds: &ArenaBounds,
) -> Vec2 {
    while let Some(&(seq, _)) = history.front() {
        if seq <= ack {
            history.pop_front();
        } else {
            break;
        }
    }
    let mut pos = confirmed;
    for &(_, dir) in history.iter() {
        pos = step_player(pos, dir, FIXED_DT, map, bounds);
    }
    pos
}

/// Asks the server to fire when the player presses Space. The server picks the
/// direction from the player's tracked facing.
fn send_shoot_request(mut commands: Commands, input: Res<ButtonInput<KeyCode>>) {
    if input.just_pressed(KeyCode::Space) {
        commands.client_trigger(ShootRequest);
    }
}

/// Sends shield press/release events only on state changes. The server mirrors
/// this into the player's [`ShieldState::requested`] flag.
fn send_shield_request(
    mut commands: Commands,
    input: Res<ButtonInput<KeyCode>>,
    mut last: Local<bool>,
) {
    let pressed = input.pressed(KeyCode::ShiftLeft) || input.pressed(KeyCode::ShiftRight);
    if pressed != *last {
        commands.client_trigger(ShieldRequest { active: pressed });
        *last = pressed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::map::TileMap;

    fn open_world() -> (TileMap, ArenaBounds) {
        // A wall-free strip so step_player just integrates motion.
        let map = TileMap::parse("xxxxxxxxx");
        let bounds = map.bounds();
        (map, bounds)
    }

    fn history(entries: &[(u32, Vec2)]) -> VecDeque<(u32, Vec2)> {
        entries.iter().copied().collect()
    }

    #[test]
    fn reconcile_drops_acked_inputs_and_replays_the_rest() {
        let (map, bounds) = open_world();
        let east = Vec2::X;
        let mut hist = history(&[(1, east), (2, east), (3, east)]);

        // Server confirmed the position after input 2; only input 3 should replay.
        let predicted = reconcile_pos(Vec2::ZERO, 2, &mut hist, &map, &bounds);

        let expected = step_player(Vec2::ZERO, east, FIXED_DT, &map, &bounds);
        assert!((predicted - expected).length() < 1e-4);
        assert_eq!(hist.len(), 1, "acked inputs should be pruned");
        assert_eq!(hist.front().unwrap().0, 3);
    }

    #[test]
    fn reconcile_with_all_acked_returns_confirmed_position() {
        let (map, bounds) = open_world();
        let mut hist = history(&[(1, Vec2::X), (2, Vec2::X)]);
        let confirmed = Vec2::new(12.0, -5.0);
        let predicted = reconcile_pos(confirmed, 5, &mut hist, &map, &bounds);
        assert_eq!(
            predicted, confirmed,
            "no unacked inputs ⇒ predicted == confirmed"
        );
        assert!(hist.is_empty());
    }

    #[test]
    fn reconcile_anchors_to_confirmed_on_misprediction() {
        let (map, bounds) = open_world();
        // History all acked, but the server reports a surprising position (e.g. the
        // player was blocked server-side). Prediction must jump to the confirmed.
        let mut hist = history(&[(1, Vec2::X)]);
        let surprising = Vec2::new(40.0, 0.0);
        let predicted = reconcile_pos(surprising, 1, &mut hist, &map, &bounds);
        assert_eq!(predicted, surprising);
    }

    #[test]
    fn reconcile_replay_matches_repeated_stepping() {
        // The core invariant: replaying N inputs == stepping N times the way the
        // server does (both go through step_player with the same FIXED_DT).
        let (map, bounds) = open_world();
        let east = Vec2::X;
        let mut hist = history(&[(1, east), (2, east), (3, east)]);
        let predicted = reconcile_pos(Vec2::ZERO, 0, &mut hist, &map, &bounds);

        let mut server = Vec2::ZERO;
        for _ in 0..3 {
            server = step_player(server, east, FIXED_DT, &map, &bounds);
        }
        assert!((predicted - server).length() < 1e-4);
    }
}
