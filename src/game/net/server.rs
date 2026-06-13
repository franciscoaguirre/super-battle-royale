//! Server-side networking: accepts connections, spawns an authoritative player
//! per client, and applies the input it receives.
//!
//! Enemies are spawned by the regular [`EnemyPlugin`](crate::game::enemy) on the
//! authoritative side, so this module only deals with players and transport.

use std::net::{SocketAddr, UdpSocket};
use std::time::SystemTime;

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::{
    RenetChannelsExt, RenetServer, RepliconRenetPlugins,
    netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig},
    renet::ConnectionConfig,
};

use super::{NetPos, PROTOCOL_ID, PlayerInput, register_protocol};
use crate::game::map::CurrentMap;
use crate::game::player::{Player, PlayerColor, PlayerIntent};

/// Maximum simultaneous players.
const MAX_CLIENTS: usize = 64;

/// The address the server binds its UDP socket to.
#[derive(Resource, Clone, Copy)]
struct BindAddr(SocketAddr);

/// Runs the headless authoritative server.
pub struct ServerNetPlugin {
    pub bind_addr: SocketAddr,
}

impl Plugin for ServerNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RepliconPlugins, RepliconRenetPlugins));
        register_protocol(app);
        app.insert_resource(BindAddr(self.bind_addr))
            .add_systems(Startup, setup_server)
            // A client is `AuthorizedClient` once its protocol hash matches ours.
            .add_observer(on_client_authorized)
            .add_observer(receive_input);
    }
}

/// Creates the renet server + netcode transport bound to the configured address.
fn setup_server(
    mut commands: Commands,
    channels: Res<RepliconChannels>,
    bind: Res<BindAddr>,
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
        protocol_id: PROTOCOL_ID,
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
fn on_client_authorized(
    add: On<Add, AuthorizedClient>,
    mut commands: Commands,
    map: Res<CurrentMap>,
    players: Query<(), With<Player>>,
) {
    let index = players.iter().count();
    let spawns = map.0.spawn_points();
    let position = if spawns.is_empty() {
        Vec2::ZERO
    } else {
        spawns[index % spawns.len()]
    };
    let color = PlayerColor::nth(index);

    commands.entity(add.entity).insert((
        Player,
        color,
        NetPos(position),
        PlayerIntent::default(),
        Replicated,
    ));
    info!(
        "player joined as {color:?} at {position:?} (entity {})",
        add.entity
    );
}

/// Applies movement input to the sending client's player.
fn receive_input(input: On<FromClient<PlayerInput>>, mut players: Query<&mut PlayerIntent>) {
    if let Some(entity) = input.client_id.entity()
        && let Ok(mut intent) = players.get_mut(entity)
    {
        intent.0 = input.dir;
    }
}
