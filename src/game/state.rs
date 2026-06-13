use bevy::prelude::*;

/// High-level game state machine.
///
/// The skeleton only implements `Playing`, but the state is already wired so
/// future screens (menu, loading, game over) can be added without restructuring.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, States)]
pub enum GameState {
    #[default]
    Playing,
}
