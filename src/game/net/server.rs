//! Server-side networking: accepts connections, spawns an authoritative player
//! per client, and applies the input it receives.
//!
//! Enemies are spawned by the regular [`BotPlugin`](crate::game::bot) on the
//! authoritative side, so this module only deals with players and transport.

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::time::SystemTime;

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon::shared::backend::connected_client::NetworkId;
use bevy_replicon_renet::{
    RenetChannelsExt, RenetServer, RepliconRenetPlugins,
    netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig},
    renet::ConnectionConfig,
};

use super::{
    ControllingClient, LastProcessedInput, MatchInfo, NetPos, Owner, PlayerInput, ShootRequest,
    StartMatch, YouAreOwner, is_server, protocol_id_for, register_protocol,
};
use crate::game::combat::{Dead, DoubleShot, QuadShot, Zigzag};
use crate::game::map::{self, CurrentMap};
use crate::game::player::{Player, PlayerColor, PlayerIntent, apply_player_intent};
use crate::game::projectile::{Facing, FireCooldown, ShotMods, try_fire};
use crate::game::state::{GameState, MatchConfig};

/// Maximum simultaneous players.
const MAX_CLIENTS: usize = 64;

/// Most inputs buffered per player before the oldest is dropped. Bounds how far
/// behind real time a flooding/lagging client can push the server.
const INPUT_QUEUE_CAP: usize = 8;

/// Server-only per-player buffer of inputs awaiting their fixed tick. One input
/// is dequeued and applied per tick (see [`dequeue_inputs`]) so the count of
/// applied inputs matches the client's one-replay-per-input reconciliation.
#[derive(Component, Default)]
struct InputQueue {
    pending: VecDeque<(u32, Vec2)>,
    /// Highest seq accepted so far; drops duplicates/out-of-order resends.
    last_enqueued: u32,
}

impl InputQueue {
    fn push(&mut self, seq: u32, dir: Vec2) {
        if seq <= self.last_enqueued {
            return; // duplicate or stale (unreliable channel can reorder)
        }
        self.last_enqueued = seq;
        if self.pending.len() >= INPUT_QUEUE_CAP {
            self.pending.pop_front();
        }
        self.pending.push_back((seq, dir));
    }
}

/// The address the server binds its UDP socket to.
#[derive(Resource, Clone, Copy)]
struct BindAddr(SocketAddr);

/// The netcode protocol id derived from the server's join code. Clients must
/// supply the same code to compute a matching id and be allowed to connect.
#[derive(Resource, Clone, Copy)]
struct ServerProtocolId(u64);

/// Runs the headless authoritative server.
pub struct ServerNetPlugin {
    pub bind_addr: SocketAddr,
    /// Join code (from the `JOIN_CODE` env var); gates connection via the
    /// protocol id. Empty means an open server.
    pub join_code: String,
}

impl Plugin for ServerNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RepliconPlugins, RepliconRenetPlugins));
        register_protocol(app);
        app.insert_resource(BindAddr(self.bind_addr))
            .insert_resource(ServerProtocolId(protocol_id_for(&self.join_code)))
            .add_systems(Startup, setup_server)
            // Place lobby-joined players at spawn points once the map is known.
            .add_systems(
                OnEnter(GameState::Playing),
                position_players.run_if(is_server),
            )
            // Apply one buffered input per fixed tick, before movement consumes it.
            .add_systems(
                FixedUpdate,
                dequeue_inputs
                    .run_if(in_state(GameState::Playing))
                    .run_if(is_server)
                    .before(apply_player_intent),
            )
            // A client is `AuthorizedClient` once its protocol hash matches ours.
            .add_observer(on_client_authorized)
            .add_observer(on_start_match)
            .add_observer(receive_input)
            .add_observer(receive_shoot);
    }
}

/// Creates the renet server + netcode transport bound to the configured address.
fn setup_server(
    mut commands: Commands,
    channels: Res<RepliconChannels>,
    bind: Res<BindAddr>,
    protocol: Res<ServerProtocolId>,
) -> Result<()> {
    let server = RenetServer::new(ConnectionConfig {
        server_channels_config: channels.server_configs(),
        client_channels_config: channels.client_configs(),
        ..Default::default()
    });

    let current_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
    let socket = UdpSocket::bind(bind.0)?;
    let server_config = ServerConfig {
        current_time,
        max_clients: MAX_CLIENTS,
        protocol_id: protocol.0,
        authentication: ServerAuthentication::Unsecure,
        public_addresses: Default::default(),
    };
    let transport = NetcodeServerTransport::new(server_config, socket)?;

    commands.insert_resource(server);
    commands.insert_resource(transport);
    info!("server listening on {}", bind.0);

    Ok(())
}

/// Spawns an authoritative player on the client's entity once it is authorized.
/// Because the player components live on the client entity itself, the renet
/// backend despawns them automatically when the client disconnects, propagating
/// the removal to every other client.
///
/// The position is left at the origin here and assigned by [`position_players`]
/// when the match starts, since the map (and thus its spawn points) isn't chosen
/// until the owner starts the match. The first client to join is tagged [`Owner`]
/// and told so via a [`YouAreOwner`] event.
fn on_client_authorized(
    add: On<Add, AuthorizedClient>,
    mut commands: Commands,
    players: Query<(), With<Player>>,
    network_ids: Query<&NetworkId>,
) {
    let index = players.iter().count();
    let color = PlayerColor::nth(index);
    // The renet backend put `NetworkId` on this same (client) entity on connect.
    // The controlling client matches it against its own id to find this player.
    let client_id = network_ids.get(add.entity).map(NetworkId::get).unwrap_or(0);

    commands.entity(add.entity).insert((
        Player,
        color,
        NetPos(Vec2::ZERO),
        PlayerIntent::default(),
        ControllingClient(client_id),
        LastProcessedInput(0),
        InputQueue::default(),
        Replicated,
    ));

    let is_owner = index == 0;
    if is_owner {
        commands.entity(add.entity).insert(Owner);
        commands.server_trigger(ToClients {
            targets: SendTargets::Single(add.entity.into()),
            message: YouAreOwner,
        });
    }
    info!(
        "player joined as {color:?}{} (entity {})",
        if is_owner { " (owner)" } else { "" },
        add.entity
    );
}

/// Positions every player at a map spawn point when the match begins. Runs only
/// on the dedicated server (offline positions its single local player in
/// `spawn_player`); the map resource is guaranteed present because the start flow
/// inserts it before transitioning to `Playing`.
fn position_players(map: Res<CurrentMap>, mut players: Query<&mut NetPos, With<Player>>) {
    let spawns = map.0.spawn_points();
    if spawns.is_empty() {
        return;
    }
    for (index, mut pos) in players.iter_mut().enumerate() {
        pos.0 = spawns[index % spawns.len()];
    }
}

/// Starts the match when the owner requests it: validates the sender owns
/// [`Owner`], records the chosen [`MatchConfig`], loads the map, spawns the
/// replicated [`MatchInfo`] singleton (the clients' "match started" signal), and
/// transitions to `Playing`. Inserting the map resources *before* the transition
/// is required: the `OnEnter(Playing)` spawn systems read them.
fn on_start_match(
    req: On<FromClient<StartMatch>>,
    owners: Query<(), With<Owner>>,
    started: Query<(), With<MatchInfo>>,
    mut commands: Commands,
    mut config: ResMut<MatchConfig>,
    mut next: ResMut<NextState<GameState>>,
) {
    let Some(entity) = req.client_id.entity() else {
        return;
    };
    // Only the owner may start, and only once.
    if owners.get(entity).is_err() || !started.is_empty() {
        return;
    }

    config.map_index = req.map_index;
    config.bot_count = req.bot_count;
    map::insert_map_resources(&mut commands, req.map_index);
    commands.spawn((
        MatchInfo {
            map_index: req.map_index,
        },
        Replicated,
    ));
    next.set(GameState::Playing);
    info!(
        "owner started match: map {} with {} bots",
        req.map_index, req.bot_count
    );
}

/// Buffers a movement input on the sending client's player. The input is applied
/// later, one per fixed tick, by [`dequeue_inputs`] — not immediately — so the
/// server consumes exactly one input per tick to match client reconciliation.
fn receive_input(input: On<FromClient<PlayerInput>>, mut players: Query<&mut InputQueue>) {
    if let Some(entity) = input.client_id.entity()
        && let Ok(mut queue) = players.get_mut(entity)
    {
        queue.push(input.seq, input.dir);
    }
}

/// Each fixed tick, applies the next buffered input per player: sets the movement
/// intent and records the applied seq in the replicated [`LastProcessedInput`]
/// (the ack the client reconciles against). On an empty queue the player coasts
/// (keeps the last intent) and the ack is left unchanged.
fn dequeue_inputs(
    mut players: Query<(&mut InputQueue, &mut PlayerIntent, &mut LastProcessedInput)>,
) {
    for (mut queue, mut intent, mut ack) in &mut players {
        if let Some((seq, dir)) = queue.pending.pop_front() {
            intent.0 = dir;
            ack.0 = seq;
        }
    }
}

/// Fires a shot for the sending client's player, in its tracked facing.
/// Dead players (awaiting respawn) can't shoot.
#[allow(clippy::type_complexity)]
fn receive_shoot(
    request: On<FromClient<ShootRequest>>,
    mut commands: Commands,
    mut players: Query<
        (
            &NetPos,
            &Facing,
            &mut FireCooldown,
            &PlayerColor,
            Option<&DoubleShot>,
            Option<&QuadShot>,
            Option<&Zigzag>,
        ),
        Without<Dead>,
    >,
) {
    if let Some(entity) = request.client_id.entity()
        && let Ok((pos, facing, mut cooldown, color, double, quad, zigzag)) =
            players.get_mut(entity)
    {
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
