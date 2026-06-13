pub mod config;
pub mod input;
pub mod lobby;

use bevy::prelude::*;
use bevy_ggrs::ReadInputs;

use crate::game::state::AppState;
use input::read_local_inputs;
use lobby::{lobby_system, log_ggrs_events, start_matchbox_socket};

pub struct NetworkingPlugin;

impl Plugin for NetworkingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(ReadInputs, read_local_inputs)
            .add_systems(OnEnter(AppState::Lobby), start_matchbox_socket)
            .add_systems(Update, lobby_system.run_if(in_state(AppState::Lobby)))
            .add_systems(Update, log_ggrs_events.run_if(in_state(AppState::InGame)));
    }
}
