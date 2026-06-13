use bevy::prelude::*;

/// High-level application state machine.
///
/// The game starts in `Lobby` while waiting for the matchmaking server to pair
/// players, then transitions to `InGame` once the GGRS P2P session is ready.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, States)]
pub enum AppState {
    #[default]
    Lobby,
    InGame,
}
