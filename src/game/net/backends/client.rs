use bevy::ecs::component::Mutable;
use bevy::prelude::*;
use bevy_replicon::prelude::{AppRuleExt, ClientEventAppExt, ClientTriggerExt, ServerEventAppExt};

use super::super::backend::NetworkBackend;
use super::super::protocol::{PlayerInput, ShieldRequest, ShootRequest};
use crate::game::InGame;

/// Connected client backend: renders and sends input, no local simulation.
#[derive(Resource, Copy, Clone, Default, Debug)]
pub struct ClientBackend;

impl NetworkBackend for ClientBackend {
    const NAME: &'static str = "client";
    const IS_AUTHORITATIVE: bool = false;
    const IS_CLIENT: bool = true;
    const IS_OFFLINE: bool = false;
    const IS_ONLINE_CLIENT: bool = true;

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
        // Client-local gameplay entities (e.g. visual effects) are tagged InGame;
        // replicated actors are spawned by the server and arrive through Replicon.
        commands.spawn((InGame, bundle)).id()
    }

    fn apply_movement_input(&self, commands: &mut Commands, dir: Vec2, seq: Option<u32>) {
        commands.client_trigger(PlayerInput {
            dir,
            seq: seq.unwrap_or(0),
        });
    }

    fn apply_shoot_input(&self, commands: &mut Commands) {
        commands.client_trigger(ShootRequest);
    }

    fn apply_shield_input(&self, commands: &mut Commands, active: bool) {
        commands.client_trigger(ShieldRequest { active });
    }
}
