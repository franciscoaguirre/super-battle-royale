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
use super_battle_royale::game::net::{
    NetPos, PROTOCOL_ID, PlayerInput, ShootRequest, register_protocol,
};
use super_battle_royale::game::player::{Player, PlayerColor};
use super_battle_royale::game::projectile::{Height, Projectile};

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

/// Set to true once the client has sent its one shoot request.
#[derive(Resource, Default)]
struct Sent(bool);

/// Drives the shoot client-event end to end: the client sends a `ShootRequest`,
/// the server reacts by spawning a projectile, and that projectile (with its
/// altitude) replicates back to the client.
#[test]
fn fires_and_replicates_projectile() {
    const SHOT_POS: Vec2 = Vec2::new(3.0, 4.0);
    const SHOT_HEIGHT: f32 = 25.0;

    let mut server_app = build_app();
    let mut client_app = build_app();

    // Server: on a shoot request, spawn a replicated projectile.
    server_app.add_observer(
        move |_req: On<FromClient<ShootRequest>>, mut commands: Commands| {
            commands.spawn((
                Projectile,
                NetPos(SHOT_POS),
                Height(SHOT_HEIGHT),
                Replicated,
            ));
        },
    );

    // Client: send exactly one shoot request once connected.
    client_app.init_resource::<Sent>();
    client_app.add_systems(
        Update,
        (|mut commands: Commands, mut sent: ResMut<Sent>| {
            if !sent.0 {
                commands.client_trigger(ShootRequest);
                sent.0 = true;
            }
        })
        .run_if(in_state(ClientState::Connected)),
    );

    let port = setup_server(&mut server_app);
    setup_client(&mut client_app, port);
    wait_for_connection(&mut server_app, &mut client_app);
    for _ in 0..100 {
        client_app.update();
        server_app.update();
    }

    let mut projectiles = client_app
        .world_mut()
        .query_filtered::<&Height, With<Projectile>>();
    let height = projectiles
        .single(client_app.world())
        .expect("client should see exactly one replicated projectile");
    assert_eq!(height.0, SHOT_HEIGHT);
}

/// Exercises the authoritative combat loop directly (no networking): a shot
/// damages a non-owner player by a fixed amount, never damages its owner, and a
/// player reaching zero health is marked `Dead`.
#[test]
fn projectile_damages_and_kills_non_owner() {
    use super_battle_royale::game::combat::{CombatPlugin, Dead, Health};
    use super_battle_royale::game::map::{CurrentMap, TileMap};
    use super_battle_royale::game::net::NetRole;
    use super_battle_royale::game::player::Player;
    use super_battle_royale::game::projectile::{Impact, ImpactKind, Projectile, ProjectileOwner};
    use super_battle_royale::game::state::GameState;

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin, CombatPlugin));
    app.init_state::<GameState>();
    app.insert_resource(NetRole::Server);
    app.insert_resource(CurrentMap(TileMap::parse("wsw")));

    // Two players on the same spot: the shooter (owner) and the target.
    let shooter = app.world_mut().spawn((Player, NetPos(Vec2::ZERO))).id();
    let target = app.world_mut().spawn((Player, NetPos(Vec2::ZERO))).id();

    // First tick gives both players full health.
    app.update();
    assert_eq!(app.world().get::<Health>(target).unwrap().current, 100.0);

    // Each shot owned by the shooter deals 25 damage to the target only.
    for expected in [75.0, 50.0, 25.0] {
        app.world_mut()
            .spawn((Projectile, ProjectileOwner(shooter), NetPos(Vec2::ZERO)));
        app.update();
        assert_eq!(app.world().get::<Health>(target).unwrap().current, expected);
        assert!(app.world().get::<Dead>(target).is_none());
    }
    assert_eq!(
        app.world().get::<Health>(shooter).unwrap().current,
        100.0,
        "a shot must never damage its owner"
    );

    // The fourth shot drops the target to zero and marks it dead.
    app.world_mut()
        .spawn((Projectile, ProjectileOwner(shooter), NetPos(Vec2::ZERO)));
    app.update();
    assert!(
        app.world().get::<Dead>(target).is_some(),
        "target should be Dead at 0 HP"
    );

    // Hits spawn an "object" impact marker (which drives the hit-object sound).
    let mut impacts = app.world_mut().query::<&Impact>();
    assert!(
        impacts
            .iter(app.world())
            .any(|impact| impact.0 == ImpactKind::Object),
        "a player hit should spawn an Object impact"
    );
}

/// Exercises the authoritative combat loop against an enemy: a player-owned
/// shot damages an enemy, and the enemy dies and respawns after the delay.
#[test]
fn projectile_damages_and_kills_enemy() {
    use super_battle_royale::game::combat::{CombatPlugin, Dead, Health};
    use super_battle_royale::game::map::{CurrentMap, TileMap};
    use super_battle_royale::game::net::NetRole;
    use super_battle_royale::game::player::Player;
    use super_battle_royale::game::projectile::{Impact, ImpactKind, Projectile, ProjectileOwner};
    use super_battle_royale::game::state::GameState;

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin, CombatPlugin));
    app.init_state::<GameState>();
    app.insert_resource(NetRole::Server);
    app.insert_resource(CurrentMap(TileMap::parse("wsw")));
    // Drive time in fixed steps so the respawn timer is deterministic.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f32(1.0 / 60.0),
    ));

    let shooter = app.world_mut().spawn((Player, NetPos(Vec2::ZERO))).id();
    let bot = app.world_mut().spawn((Enemy, NetPos(Vec2::ZERO))).id();

    // First tick gives the enemy full health.
    app.update();
    assert_eq!(app.world().get::<Health>(bot).unwrap().current, 100.0);

    // Each shot owned by the player deals 25 damage to the enemy.
    for expected in [75.0, 50.0, 25.0] {
        app.world_mut()
            .spawn((Projectile, ProjectileOwner(shooter), NetPos(Vec2::ZERO)));
        app.update();
        assert_eq!(app.world().get::<Health>(bot).unwrap().current, expected);
        assert!(app.world().get::<Dead>(bot).is_none());
    }

    // The fourth shot drops the enemy to zero and marks it dead.
    app.world_mut()
        .spawn((Projectile, ProjectileOwner(shooter), NetPos(Vec2::ZERO)));
    app.update();
    assert!(
        app.world().get::<Dead>(bot).is_some(),
        "enemy should be Dead at 0 HP"
    );

    // Hits spawn an "object" impact marker.
    let mut impacts = app.world_mut().query::<&Impact>();
    assert!(
        impacts
            .iter(app.world())
            .any(|impact| impact.0 == ImpactKind::Object),
        "an enemy hit should spawn an Object impact"
    );

    // Step through the respawn delay; the enemy should come back to life.
    for _ in 0..240 {
        app.update();
    }
    assert!(
        app.world().get::<Dead>(bot).is_none(),
        "enemy should respawn and lose Dead marker"
    );
    assert_eq!(
        app.world().get::<Health>(bot).unwrap().current,
        100.0,
        "enemy should respawn with full health"
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
