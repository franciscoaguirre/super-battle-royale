//! Headless integration test for the multiplayer protocol.
//!
//! Runs a server `App` and a client `App` in one process (no rendering), connects
//! them over a real loopback UDP socket, and asserts that the registered protocol
//! replicates players and enemies to the client and that client input reaches the
//! server. Requires both networking sides, so run with:
//!
//! ```bash
//! cargo test --features server
//! ```
#![cfg(all(feature = "client", feature = "server"))]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::SystemTime;

use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::{
    RenetChannelsExt, RenetClient, RenetServer, RepliconRenetPlugins,
    netcode::{
        ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication,
        ServerConfig,
    },
    renet::ConnectionConfig,
};

use super_battle_royale::game::enemy::Enemy;
use super_battle_royale::game::net::{NetPos, PROTOCOL_ID, PlayerInput, register_protocol};
use super_battle_royale::game::player::{Player, PlayerColor};

const PLAYER_POS: Vec2 = Vec2::new(12.0, -34.0);
const ENEMY_POS: Vec2 = Vec2::new(-5.0, 7.0);
const TEST_INPUT: Vec2 = Vec2::new(1.0, 0.0);

/// Records the most recent input the server received from a client.
#[derive(Resource, Default)]
struct ReceivedInput(Option<Vec2>);

#[test]
fn replicates_world_and_receives_input() {
    let mut server_app = build_app();
    let mut client_app = build_app();

    // Server side: capture client input and stand up an authoritative world.
    server_app.init_resource::<ReceivedInput>();
    server_app.add_observer(
        |input: On<FromClient<PlayerInput>>, mut received: ResMut<ReceivedInput>| {
            received.0 = Some(input.dir);
        },
    );
    server_app
        .world_mut()
        .spawn((Player, PlayerColor::Red, NetPos(PLAYER_POS), Replicated));
    server_app
        .world_mut()
        .spawn((Enemy, NetPos(ENEMY_POS), Replicated));

    // Client side: stream a fixed input once connected.
    client_app.add_systems(
        Update,
        (|mut commands: Commands| commands.client_trigger(PlayerInput { dir: TEST_INPUT }))
            .run_if(in_state(ClientState::Connected)),
    );

    // Connect over loopback UDP and let both sides settle.
    let port = setup_server(&mut server_app);
    setup_client(&mut client_app, port);
    wait_for_connection(&mut server_app, &mut client_app);
    for _ in 0..100 {
        client_app.update();
        server_app.update();
    }

    // The player replicated with its color and exact position.
    let mut players = client_app
        .world_mut()
        .query_filtered::<(&PlayerColor, &NetPos), With<Player>>();
    let (color, pos) = players
        .single(client_app.world())
        .expect("client should see exactly one player");
    assert_eq!(*color, PlayerColor::Red);
    assert_eq!(pos.0, PLAYER_POS);

    // The enemy replicated with its exact position.
    let mut enemies = client_app
        .world_mut()
        .query_filtered::<&NetPos, With<Enemy>>();
    let enemy_pos = enemies
        .single(client_app.world())
        .expect("client should see exactly one enemy");
    assert_eq!(enemy_pos.0, ENEMY_POS);

    // The client's input reached the server.
    assert_eq!(
        server_app.world().resource::<ReceivedInput>().0,
        Some(TEST_INPUT),
        "server should have received the client's input"
    );
}

/// Builds a headless app with the networking stack and our protocol registered.
fn build_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        // Replicate every frame so the manual stepping below is deterministic.
        RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
        RepliconRenetPlugins,
    ));
    register_protocol(&mut app);
    app.finish();
    app
}

fn setup_server(app: &mut App) -> u16 {
    let channels = app.world().resource::<RepliconChannels>();
    let server = RenetServer::new(ConnectionConfig {
        server_channels_config: channels.server_configs(),
        client_channels_config: channels.client_configs(),
        ..Default::default()
    });

    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let public_addr = socket.local_addr().unwrap();
    let port = public_addr.port();
    let server_config = ServerConfig {
        current_time,
        max_clients: 1,
        protocol_id: PROTOCOL_ID,
        public_addresses: vec![public_addr],
        authentication: ServerAuthentication::Unsecure,
    };
    let transport = NetcodeServerTransport::new(server_config, socket).unwrap();

    app.insert_resource(server).insert_resource(transport);
    port
}

fn setup_client(app: &mut App, port: u16) {
    let channels = app.world().resource::<RepliconChannels>();
    let client = RenetClient::new(ConnectionConfig {
        server_channels_config: channels.server_configs(),
        client_channels_config: channels.client_configs(),
        ..Default::default()
    });

    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let authentication = ClientAuthentication::Unsecure {
        client_id: 1,
        protocol_id: PROTOCOL_ID,
        server_addr,
        user_data: None,
    };
    let transport = NetcodeClientTransport::new(current_time, authentication, socket).unwrap();

    app.insert_resource(client).insert_resource(transport);
}

fn wait_for_connection(server_app: &mut App, client_app: &mut App) {
    for _ in 0..1000 {
        client_app.update();
        server_app.update();
        if client_app.world().resource::<RenetClient>().is_connected() {
            return;
        }
    }
    panic!("client failed to connect to server");
}
