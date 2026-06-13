use bevy::prelude::*;
use bevy_ggrs::prelude::*;
use bevy_matchbox::prelude::*;

use crate::args::Args;
use crate::game::state::AppState;
use crate::networking::config::SbrConfig;

pub fn start_matchbox_socket(mut commands: Commands, args: Res<Args>) {
    let room_id = match &args.room {
        Some(id) => id.clone(),
        None => format!("sbr?next={}", args.players),
    };
    let room_url = format!("{}/{}", args.matchbox, room_id);
    info!("Connecting to matchbox server: {room_url}");

    commands.insert_resource(MatchboxSocket::new_unreliable(room_url));
}

pub fn lobby_system(
    mut commands: Commands,
    mut socket: ResMut<MatchboxSocket>,
    args: Res<Args>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let Ok(peer_changes) = socket.try_update_peers() else {
        warn!("Socket dropped");
        return;
    };

    for (peer, new_state) in peer_changes {
        match new_state {
            PeerState::Connected => info!("Peer {peer} connected"),
            PeerState::Disconnected => info!("Peer {peer} disconnected"),
        }
    }

    let connected_peers = socket.connected_peers().count();
    let remaining = args.players.saturating_sub(connected_peers + 1);
    if remaining > 0 {
        return;
    }

    info!("All peers joined, starting game");

    let players = socket.players();

    let mut sess_build = SessionBuilder::<SbrConfig>::new()
        .with_num_players(args.players)
        .with_max_prediction_window(12)
        .with_input_delay(2);

    for (i, player) in players.into_iter().enumerate() {
        sess_build = sess_build
            .add_player(player, i)
            .expect("failed to add player");
    }

    let channel = socket.take_channel(0).expect("missing channel");
    let sess = sess_build
        .start_p2p_session(channel)
        .expect("failed to start session");

    commands.insert_resource(Session::P2P(sess));
    next_state.set(AppState::InGame);
}

pub fn log_ggrs_events(mut session: ResMut<Session<SbrConfig>>) {
    match session.as_mut() {
        Session::P2P(s) => {
            for event in s.events() {
                match event {
                    GgrsEvent::Disconnected { .. } | GgrsEvent::NetworkInterrupted { .. } => {
                        warn!("GGRS event: {event:?}")
                    }
                    GgrsEvent::DesyncDetected { .. } => error!("GGRS event: {event:?}"),
                    _ => info!("GGRS event: {event:?}"),
                }
            }
        }
        _ => {}
    }
}
