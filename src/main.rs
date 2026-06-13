use bevy::prelude::*;
use bevy_ggrs::prelude::*;
use clap::Parser;

use super_battle_royale::args::Args;
use super_battle_royale::game::state::AppState;
use super_battle_royale::networking::config::SbrConfig;
use super_battle_royale::{GamePlugin, NetworkingPlugin};

const FPS: usize = 60;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Super Battle Royale".into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(GgrsPlugin::<SbrConfig>::default())
    .insert_resource(RollbackFrameRate(FPS))
    .rollback_component_with_clone::<Transform>()
    .insert_resource(args.clone())
    .add_plugins((GamePlugin, NetworkingPlugin));

    if args.synctest {
        let mut sess_build = SessionBuilder::<SbrConfig>::new()
            .with_num_players(args.players)
            .with_check_distance(7)
            .with_input_delay(2);

        for i in 0..args.players {
            sess_build = sess_build.add_player(PlayerType::Local, i)?;
        }

        let sess = sess_build.start_synctest_session()?;
        app.insert_resource(Session::SyncTest(sess));
        app.insert_state(AppState::InGame);
    } else {
        app.insert_state(AppState::Lobby);
    }

    app.run();

    Ok(())
}
