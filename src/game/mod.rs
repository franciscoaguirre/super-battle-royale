pub mod bot;
pub mod combat;
pub mod map;
pub mod match_flow;
pub mod music;
pub mod net;
pub mod pickup;
pub mod player;
pub mod projectile;
pub mod shield;
pub mod state;

// Rendering-only subsystems live in the windowed client; the headless server
// never compiles them.
#[cfg(feature = "client")]
pub mod camera;
#[cfg(feature = "client")]
pub mod crt;
#[cfg(feature = "client")]
pub mod effects;
#[cfg(feature = "client")]
pub mod footsteps;
#[cfg(feature = "client")]
pub mod lobby;
#[cfg(feature = "client")]
pub mod ping;

use bevy::prelude::*;

use net::{InputBackend, NetworkAppExt, SpawnContext, init_network_registry};
use state::{GameState, MatchConfig};

/// Top-level plugin that wires up the entire game.
///
/// This is shared by both binaries. Simulation plugins (`bot`, `map`,
/// `player`) compile everywhere; the simulation runs only where this instance is
/// authoritative (see [`net::is_authoritative`]). Rendering/audio plugins are
/// added only in the client build. The networking transport itself
/// ([`net::client`]/[`net::server`]) is added by each binary, since it depends on
/// the chosen [`net::NetRole`].
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        init_network_registry(app);
        app.register_networked::<net::NetPos>()
            .init_state::<GameState>()
            .add_systems(
                Startup,
                init_spawn_context.run_if(resource_exists::<net::NetRole>),
            )
            .add_systems(
                Startup,
                init_input_backend.run_if(resource_exists::<net::NetRole>),
            )
            .init_resource::<MatchConfig>()
            // Run the fixed simulation step (player movement + prediction) at the
            // server's 60 Hz loop rate, so client replay matches server steps.
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .add_plugins((
                combat::CombatPlugin,
                bot::BotPlugin,
                map::MapPlugin,
                match_flow::MatchFlowPlugin,
                pickup::PickupPlugin,
                player::PlayerPlugin,
                projectile::ProjectilePlugin,
                shield::ShieldPlugin,
            ))
            // Cleanup runs when LEAVING the GameOver announcement (the map-switch
            // point), not on leaving Playing — so the scene stays frozen and
            // visible behind the winner banner during GameOver.
            .add_systems(OnExit(GameState::GameOver), cleanup_ingame);

        #[cfg(feature = "client")]
        app.add_plugins((
            camera::CameraPlugin,
            crt::CrtPlugin,
            effects::EffectsPlugin,
            footsteps::FootstepsPlugin,
            lobby::LobbyPlugin,
            music::MusicPlugin,
            ping::PingPlugin,
        ))
        .init_resource::<net::LatestLocalInput>()
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

/// Mirrors the startup [`NetRole`] into a [`SpawnContext`] resource so gameplay
/// spawn systems can decide replication/`InGame` policy without querying the role
/// directly. Runs only when a role has been inserted by the binary.
fn init_spawn_context(mut commands: Commands, role: Res<net::NetRole>) {
    commands.insert_resource(SpawnContext { role: *role });
}

/// Mirrors the startup [`NetRole`] into an [`InputBackend`] resource so input
/// systems can route local player input without knowing the exact role.
fn init_input_backend(mut commands: Commands, role: Res<net::NetRole>) {
    commands.insert_resource(InputBackend::from_role(*role));
}
