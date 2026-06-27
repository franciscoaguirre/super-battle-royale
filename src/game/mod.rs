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

use std::marker::PhantomData;

use bevy::prelude::*;

use net::NetworkBackend;
use state::{GameState, MatchConfig};

/// Top-level plugin that wires up the entire game.
///
/// This is shared by both binaries. Simulation plugins (`bot`, `map`, `player`)
/// compile everywhere; the simulation runs only where this instance is
/// authoritative (see [`NetworkBackend::IS_AUTHORITATIVE`]). Rendering/audio
/// plugins are added only in the client build. The networking transport itself
/// ([`net::client`]/[`net::server`]) is added by each binary.
pub struct GamePlugin<B: NetworkBackend> {
    _backend: PhantomData<B>,
}

impl<B: NetworkBackend> GamePlugin<B> {
    pub fn new() -> Self {
        Self {
            _backend: PhantomData,
        }
    }
}

impl<B: NetworkBackend> Plugin for GamePlugin<B> {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .init_resource::<MatchConfig>()
            // Run the fixed simulation step (player movement + prediction) at the
            // server's 60 Hz loop rate, so client replay matches server steps.
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .add_plugins((
                combat::CombatPlugin::<B>::new(),
                bot::BotPlugin::<B>::new(),
                map::MapPlugin,
                match_flow::MatchFlowPlugin::<B>::new(),
                pickup::PickupPlugin::<B>::new(),
                player::PlayerPlugin::<B>::new(),
                projectile::ProjectilePlugin::<B>::new(),
                shield::ShieldPlugin::<B>::new(),
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
            footsteps::FootstepsPlugin::<B>::new(),
            lobby::LobbyPlugin::<B>::new(),
            music::MusicPlugin,
            ping::PingPlugin::<B>::new(),
        ))
        .add_systems(PostUpdate, net::sync_netpos_to_transform::<B>);
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
