use bevy::ecs::component::Mutable;
use bevy::prelude::*;

use super::super::backend::{NetworkBackend, NextPlayerIntent, NextShieldRequest, NextShoot};
use crate::game::InGame;

/// Local single-player backend: simulates and renders, no networking.
#[derive(Resource, Copy, Clone, Default, Debug)]
pub struct OfflineBackend;

impl NetworkBackend for OfflineBackend {
    const NAME: &'static str = "offline";
    const IS_AUTHORITATIVE: bool = true;
    const IS_CLIENT: bool = true;
    const IS_OFFLINE: bool = true;
    const IS_ONLINE_CLIENT: bool = false;

    fn register_replicated<C>(&self, _app: &mut App)
    where
        C: Component<Mutability = Mutable>
            + serde::Serialize
            + serde::de::DeserializeOwned
            + Clone
            + Send
            + Sync
            + 'static,
    {
        // Offline has no replication stack; components are local-only.
    }

    fn register_client_event<E>(&self, _app: &mut App, _channel: bevy_replicon::prelude::Channel)
    where
        E: Event + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
        for<'a> <E as Event>::Trigger<'a>: Default,
    {
    }

    fn register_server_event<E>(&self, _app: &mut App, _channel: bevy_replicon::prelude::Channel)
    where
        E: Event + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
        for<'a> <E as Event>::Trigger<'a>: Default,
    {
    }

    fn spawn_actor<B: Bundle>(&self, commands: &mut Commands, bundle: B) -> Entity {
        commands.spawn((InGame, bundle)).id()
    }

    fn apply_movement_input(&self, commands: &mut Commands, dir: Vec2, _seq: Option<u32>) {
        commands.insert_resource(NextPlayerIntent(dir));
    }

    fn apply_shoot_input(&self, commands: &mut Commands) {
        commands.insert_resource(NextShoot(true));
    }

    fn apply_shield_input(&self, commands: &mut Commands, active: bool) {
        commands.insert_resource(NextShieldRequest(active));
    }
}
