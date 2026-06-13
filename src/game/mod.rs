pub mod arena;
pub mod camera;
pub mod enemy;
pub mod player;
pub mod state;

use bevy::prelude::*;

use state::AppState;

/// Top-level plugin that wires up the entire game.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            .add_plugins((
                arena::ArenaPlugin,
                camera::CameraPlugin,
                player::PlayerPlugin,
            ))
            .add_systems(OnExit(AppState::InGame), cleanup_ingame);
    }
}

/// Marker component for entities that belong to the active gameplay session.
/// Despawning everything with this marker makes state transitions cheap and safe.
#[derive(Component)]
pub struct InGame;

fn cleanup_ingame(mut commands: Commands, query: Query<Entity, With<InGame>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}
