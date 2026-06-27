use bevy::ecs::component::Mutable;
use bevy::prelude::*;
use bevy_replicon::prelude::{AppRuleExt, ClientEventAppExt, Replicated, ServerEventAppExt};

use super::super::backend::NetworkBackend;
use crate::game::InGame;

/// Headless dedicated server backend: simulates and replicates, no rendering.
#[derive(Resource, Copy, Clone, Default, Debug)]
pub struct ServerBackend;

impl NetworkBackend for ServerBackend {
    const NAME: &'static str = "server";
    const IS_AUTHORITATIVE: bool = true;
    const IS_CLIENT: bool = false;
    const IS_OFFLINE: bool = false;
    const IS_ONLINE_CLIENT: bool = false;

    fn register_replicated<C>(&self, app: &mut App)
    where
        C: Component<Mutability = Mutable>
            + serde::Serialize
            + serde::de::DeserializeOwned
            + Clone
            + Send
            + Sync
            + 'static,
    {
        app.replicate::<C>();
    }

    fn register_client_event<E>(&self, app: &mut App, channel: bevy_replicon::prelude::Channel)
    where
        E: Event + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
        for<'a> <E as Event>::Trigger<'a>: Default,
    {
        app.add_client_event::<E>(channel);
    }

    fn register_server_event<E>(&self, app: &mut App, channel: bevy_replicon::prelude::Channel)
    where
        E: Event + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
        for<'a> <E as Event>::Trigger<'a>: Default,
    {
        app.add_server_event::<E>(channel);
    }

    fn spawn_actor<B: Bundle>(&self, commands: &mut Commands, bundle: B) -> Entity {
        commands.spawn((InGame, bundle, Replicated)).id()
    }

    fn apply_movement_input(&self, _commands: &mut Commands, _dir: Vec2, _seq: Option<u32>) {}

    fn apply_shoot_input(&self, _commands: &mut Commands) {}

    fn apply_shield_input(&self, _commands: &mut Commands, _active: bool) {}
}
