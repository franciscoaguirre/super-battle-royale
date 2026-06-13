use bevy::prelude::*;
use clap::Parser;

#[derive(Parser, Resource, Debug, Clone)]
#[clap(name = "super-battle-royale", rename_all = "kebab-case")]
pub struct Args {
    /// URL of the Matchbox signaling server.
    #[clap(long, default_value = "ws://127.0.0.1:3536")]
    pub matchbox: String,

    /// Room ID to join. If omitted, a default room is used.
    #[clap(long)]
    pub room: Option<String>,

    /// Total number of players (including yourself).
    #[clap(long, short, default_value = "2")]
    pub players: usize,

    /// Run in local sync-test mode instead of connecting to a peer.
    #[clap(long)]
    pub synctest: bool,
}
