//! Client-side networking: connects to a server and forwards local input.
//!
//! Added only when the game is launched with a server address. Rendering of the
//! replicated world (sprites, position interpolation) is handled by the regular
//! gameplay plugins, gated on the `client` feature.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::SystemTime;

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::{
    RenetChannelsExt, RenetClient, RepliconRenetPlugins,
    netcode::{ClientAuthentication, NetcodeClientTransport},
    renet::ConnectionConfig,
};

use super::{PROTOCOL_ID, PlayerInput, register_protocol};
use crate::game::player::input_direction;

/// The server endpoint this client should connect to.
#[derive(Resource, Clone, Copy)]
struct ServerEndpoint(SocketAddr);

/// Connects the windowed game to a dedicated server.
pub struct ClientNetPlugin {
    pub server_addr: SocketAddr,
}

impl Plugin for ClientNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RepliconPlugins, RepliconRenetPlugins));
        register_protocol(app);
        app.insert_resource(ServerEndpoint(self.server_addr))
            .add_systems(Startup, setup_client)
            // Only stream input once the connection is established.
            .add_systems(Update, send_input.run_if(in_state(ClientState::Connected)));
    }
}

/// Creates the renet client + netcode transport from the configured endpoint.
fn setup_client(
    mut commands: Commands,
    channels: Res<RepliconChannels>,
    endpoint: Res<ServerEndpoint>,
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
        protocol_id: PROTOCOL_ID,
        server_addr: endpoint.0,
        user_data: None,
    };
    let transport = NetcodeClientTransport::new(current_time, authentication, socket)?;

    commands.insert_resource(client);
    commands.insert_resource(transport);
    info!("connecting to server at {}", endpoint.0);

    Ok(())
}

/// Sends the current movement direction to the server every frame. Unreliable:
/// a dropped packet is corrected by the next frame's send.
fn send_input(mut commands: Commands, input: Res<ButtonInput<KeyCode>>) {
    commands.client_trigger(PlayerInput {
        dir: input_direction(&input),
    });
}
