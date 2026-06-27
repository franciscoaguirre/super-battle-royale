//! Generic network backend abstraction.
//!
//! Gameplay code is generic over [`NetworkBackend`] instead of branching on a
//! runtime role. Each binary picks one backend at startup
//! and monomorphizes [`GamePlugin`](crate::game::GamePlugin) and the gameplay
//! sub-plugins for that backend.

use bevy::ecs::component::Mutable;
use bevy::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Everything the gameplay code needs from the network layer.
pub trait NetworkBackend: Resource + Copy + Send + Sync + 'static {
    /// Name for logging/debugging.
    const NAME: &'static str;

    /// Is this instance authoritative (offline + server)?
    const IS_AUTHORITATIVE: bool;

    /// Is this instance rendering (offline + online client)?
    const IS_CLIENT: bool;

    /// Is this the local single-player instance?
    const IS_OFFLINE: bool;

    /// Is this a connected online client?
    const IS_ONLINE_CLIENT: bool;

    /// Register a replicated component.
    fn register_replicated<C>(&self, app: &mut App)
    where
        C: Component<Mutability = Mutable>
            + Serialize
            + DeserializeOwned
            + Clone
            + Send
            + Sync
            + 'static;

    /// Register a client→server event.
    fn register_client_event<E>(&self, app: &mut App, channel: bevy_replicon::prelude::Channel)
    where
        E: Event + Serialize + DeserializeOwned + Send + Sync + 'static,
        for<'a> <E as Event>::Trigger<'a>: Default;

    /// Register a server→client event.
    fn register_server_event<E>(&self, app: &mut App, channel: bevy_replicon::prelude::Channel)
    where
        E: Event + Serialize + DeserializeOwned + Send + Sync + 'static,
        for<'a> <E as Event>::Trigger<'a>: Default;

    /// Spawn a gameplay entity. Offline/server apply [`InGame`](crate::game::InGame); server also
    /// marks the entity as [`Replicated`](bevy_replicon::prelude::Replicated).
    fn spawn_actor<B: Bundle>(&self, commands: &mut Commands, bundle: B) -> Entity;

    /// Route local movement input. Offline stores it for the authoritative
    /// systems; a client sends it as a network event.
    fn apply_movement_input(&self, commands: &mut Commands, dir: Vec2, seq: Option<u32>);

    /// Route a local shoot request. Offline stores it for the authoritative
    /// systems; a client sends it as a network event.
    fn apply_shoot_input(&self, commands: &mut Commands);

    /// Route a local shield request. Offline stores it for the authoritative
    /// systems; a client sends it as a network event.
    fn apply_shield_input(&self, commands: &mut Commands, active: bool);
}

/// Offline single-player movement input for the current frame. The authoritative
/// systems copy this into the local player's [`PlayerIntent`](crate::game::player::PlayerIntent).
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct NextPlayerIntent(pub Vec2);

/// Offline single-player shoot request for the current frame. Consumed by the
/// authoritative shoot system.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct NextShoot(pub bool);

/// Offline single-player shield request for the current frame. The authoritative
/// shield system copies this into the actor's [`ShieldState`](crate::game::shield::ShieldState).
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct NextShieldRequest(pub bool);
