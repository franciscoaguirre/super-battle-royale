use bevy::prelude::*;

/// High-level game state machine.
///
/// The game opens in [`GameState::Lobby`], where the owner configures the match
/// (map + bot count) and starts it; everyone then moves to [`GameState::Playing`].
/// The state is wired so further screens (loading, game over) can be added
/// without restructuring.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, States)]
pub enum GameState {
    /// Pre-match setup: the lobby UI is shown and no simulation runs yet.
    #[default]
    Lobby,
    /// The match is live: the arena, players and bots exist and simulate.
    Playing,
}

/// The configuration the match runs with: which map (index into
/// [`crate::game::map::MAPS`]) and how many bots. The lobby edits a local draft;
/// the authoritative value is set when the match starts (by the server from a
/// `StartMatch` event, or directly offline). Exists on every role.
#[derive(Resource, Clone, Copy, Debug)]
pub struct MatchConfig {
    pub map_index: u8,
    pub bot_count: u8,
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            map_index: 0,
            bot_count: 3,
        }
    }
}
