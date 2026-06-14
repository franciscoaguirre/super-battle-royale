pub mod enemy;
pub mod map;
pub mod music;
pub mod net;
pub mod player;
pub mod state;

// Rendering-only subsystems live in the windowed client; the headless server
// never compiles them.
#[cfg(feature = "client")]
pub mod camera;
#[cfg(feature = "client")]
pub mod footsteps;

use bevy::prelude::*;

use state::GameState;

/// Top-level plugin that wires up the entire game.
///
/// This is shared by both binaries. Simulation plugins (`enemy`, `map`,
/// `player`) compile everywhere; the simulation runs only where this instance is
/// authoritative (see [`net::is_authoritative`]). Rendering/audio plugins are
/// added only in the client build. The networking transport itself
/// ([`net::client`]/[`net::server`]) is added by each binary, since it depends on
/// the chosen [`net::NetRole`].
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .add_plugins((enemy::EnemyPlugin, map::MapPlugin, player::PlayerPlugin))
            .add_systems(OnExit(GameState::Playing), cleanup_ingame);

        #[cfg(feature = "client")]
        app.add_plugins((
            camera::CameraPlugin,
            footsteps::FootstepsPlugin,
            music::MusicPlugin,
        ))
        .add_systems(PostUpdate, net::sync_netpos_to_transform);
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
