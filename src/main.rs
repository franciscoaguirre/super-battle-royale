use std::net::{SocketAddr, ToSocketAddrs};

use bevy::prelude::*;
use super_battle_royale::GamePlugin;
use super_battle_royale::game::net::client::ClientNetPlugin;
use super_battle_royale::game::net::{DEFAULT_PORT, NetRole};

/// The windowed game client.
///
/// Usage:
///   super-battle-royale                 # offline single-player
///   super-battle-royale <domain[:port]> # connect to a dedicated server
fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Super Battle Royale".into(),
            ..default()
        }),
        ..default()
    }));

    match std::env::args().nth(1) {
        Some(arg) => {
            let server_addr = resolve_server(&arg)
                .unwrap_or_else(|err| panic!("could not resolve server `{arg}`: {err}"));
            info!("starting online client → {server_addr}");
            app.insert_resource(NetRole::OnlineClient)
                .add_plugins(GamePlugin)
                .add_plugins(ClientNetPlugin { server_addr });
        }
        None => {
            info!("starting offline single-player");
            app.insert_resource(NetRole::Offline)
                .add_plugins(GamePlugin);
        }
    }

    app.run();
}

/// Resolves a `domain`, `domain:port`, or `ip:port` argument to a socket address
/// via DNS, defaulting the port to [`DEFAULT_PORT`] when none is given.
fn resolve_server(arg: &str) -> std::io::Result<SocketAddr> {
    let with_port = if arg.contains(':') {
        arg.to_string()
    } else {
        format!("{arg}:{DEFAULT_PORT}")
    };
    with_port
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses resolved"))
}
