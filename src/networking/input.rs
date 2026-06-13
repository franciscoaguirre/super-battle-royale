use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_ggrs::{LocalInputs, LocalPlayers};

use crate::networking::config::{PlayerInput, SbrConfig, INPUT_DOWN, INPUT_LEFT, INPUT_RIGHT, INPUT_UP};

pub fn read_local_inputs(
    mut commands: Commands,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    local_players: Res<LocalPlayers>,
) {
    let mut local_inputs = HashMap::new();

    for handle in &local_players.0 {
        let mut input: u8 = 0;

        if keyboard_input.pressed(KeyCode::KeyW) || keyboard_input.pressed(KeyCode::ArrowUp) {
            input |= INPUT_UP;
        }
        if keyboard_input.pressed(KeyCode::KeyS) || keyboard_input.pressed(KeyCode::ArrowDown) {
            input |= INPUT_DOWN;
        }
        if keyboard_input.pressed(KeyCode::KeyA) || keyboard_input.pressed(KeyCode::ArrowLeft) {
            input |= INPUT_LEFT;
        }
        if keyboard_input.pressed(KeyCode::KeyD) || keyboard_input.pressed(KeyCode::ArrowRight) {
            input |= INPUT_RIGHT;
        }

        local_inputs.insert(*handle, PlayerInput { inp: input });
    }

    commands.insert_resource(LocalInputs::<SbrConfig>(local_inputs));
}
