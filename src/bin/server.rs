use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use super_battle_royale::GamePlugin;
use super_battle_royale::game::net::server::ServerNetPlugin;
use super_battle_royale::game::net::{DEFAULT_PORT, NetRole};

/// The headless, dedicated game server.
///
/// Usage:
///   server                  # bind 0.0.0.0:5000
///   server <port>           # bind 0.0.0.0:<port>
///   server <host:port>      # bind a specific address
fn main() {
    let bind_addr = parse_bind_addr();

    App::new()
        // Headless: a fixed 60 Hz loop with no rendering, audio, or window.
        .add_plugins(
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
                1.0 / 60.0,
            ))),
        )
        .add_plugins((LogPlugin::default(), StatesPlugin))
        .insert_resource(NetRole::Server)
        .add_plugins(GamePlugin)
        .add_plugins(ServerNetPlugin { bind_addr })
        .run();
}

/// Parses the optional bind argument: a full `host:port`, a bare port, or
/// nothing (defaulting to `0.0.0.0:DEFAULT_PORT`).
fn parse_bind_addr() -> SocketAddr {
    let default = SocketAddr::from((Ipv4Addr::UNSPECIFIED, DEFAULT_PORT));
    let Some(arg) = std::env::args().nth(1) else {
        return default;
    };

    if let Ok(addr) = arg.parse::<SocketAddr>() {
        addr
    } else if let Ok(port) = arg.parse::<u16>() {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))
    } else {
        arg.to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
            .unwrap_or(default)
    }
}
