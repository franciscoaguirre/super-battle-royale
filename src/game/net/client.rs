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

use super::{PlayerInput, ShieldRequest, ShootRequest, protocol_id_for, register_protocol};
use crate::game::player::input_direction;
use crate::game::state::GameState;

/// The server endpoint this client should connect to.
#[derive(Resource, Clone, Copy)]
struct ServerEndpoint(SocketAddr);

/// The netcode protocol id derived from the join code; must match the server's.
#[derive(Resource, Clone, Copy)]
struct ClientProtocolId(u64);

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
            .add_systems(Startup, setup_client)
            // Only send input once connected and the match is actually underway.
            .add_systems(
                Update,
                (send_input, send_shoot_request, send_shield_request)
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

/// Sends the current movement direction to the server every frame. Unreliable:
/// a dropped packet is corrected by the next frame's send.
fn send_input(mut commands: Commands, input: Res<ButtonInput<KeyCode>>) {
    commands.client_trigger(PlayerInput {
        dir: input_direction(&input),
    });
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
