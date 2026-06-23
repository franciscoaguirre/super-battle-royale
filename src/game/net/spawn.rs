//! Role-agnostic spawn helpers.
//!
//! Gameplay code should use these instead of manually inserting [`Replicated`] and
//! [`InGame`](crate::game::InGame). Each helper encodes the spawn policy for its
//! entity type: whether it replicates (only on the server), whether it is cleaned
//! up on round exit, and what extra components it needs.
//!
//! The policies match the current behavior:
//! - Players persist across online rounds; offline they are recreated each round.
//! - Bots, projectiles, and pickups are recreated every round.
//! - Impacts have their own short lifetime and are not tied to round cleanup.
//! - Map tiles, pads, and visual/audio effects are local-only and [`InGame`].

use bevy::prelude::*;
use bevy_replicon::prelude::Replicated;

use super::NetRole;
use crate::game::InGame;

/// Spawn context carrying the active [`NetRole`]. Helpers read this to decide
/// whether to insert [`Replicated`] and whether to tag entities with [`InGame`].
#[derive(Clone, Copy, Resource)]
pub struct SpawnContext {
    pub role: NetRole,
}

impl SpawnContext {
    /// Build a context from the current [`NetRole`] resource. Panics if the
    /// resource is missing; gameplay spawn systems always run after the binary
    /// has inserted it.
    pub fn from_world(world: &World) -> Self {
        Self {
            role: *world.resource::<NetRole>(),
        }
    }

    /// True when this instance is the authoritative server (so dynamic entities
    /// must carry [`Replicated`]).
    fn replicates(&self) -> bool {
        matches!(self.role, NetRole::Server)
    }

    /// True in offline single-player, where the local player is recreated each
    /// round and therefore tagged [`InGame`].
    fn offline_player_transient(&self) -> bool {
        matches!(self.role, NetRole::Offline)
    }
}

/// Resolve an optional [`SpawnContext`] resource, defaulting to an offline role
/// when the resource is missing (e.g. in unit tests that build a single plugin).
pub fn resolve_spawn_context(ctx: Option<Res<SpawnContext>>) -> SpawnContext {
    ctx.map(|c| *c).unwrap_or(SpawnContext {
        role: NetRole::Offline,
    })
}

/// Extension trait for [`Commands`] with role-agnostic spawn helpers.
pub trait SpawnCommandsExt {
    /// Spawn a player entity. Persistent on the server (no [`InGame`]); offline
    /// the local player is recreated each round and is therefore [`InGame`].
    fn spawn_player(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_>;

    /// Spawn a bot entity. Recreated every round ([`InGame`]).
    fn spawn_bot(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_>;

    /// Spawn a projectile. Recreated every round ([`InGame`]).
    fn spawn_projectile(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_>;

    /// Spawn an impact marker. Has its own lifetime, not tied to round cleanup.
    fn spawn_impact(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_>;

    /// Spawn a pickup. Recreated every round ([`InGame`]).
    fn spawn_pickup(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_>;

    /// Spawn a transient gameplay entity that should be cleaned up on round exit.
    /// Does **not** replicate (used for map tiles, pickup pads, effects, etc.).
    fn spawn_ingame(&mut self, bundle: impl Bundle) -> EntityCommands<'_>;
}

impl SpawnCommandsExt for Commands<'_, '_> {
    fn spawn_player(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_> {
        let mut e = self.spawn(bundle);
        if ctx.replicates() {
            e.insert(Replicated);
        }
        if ctx.offline_player_transient() {
            e.insert(InGame);
        }
        e
    }

    fn spawn_bot(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_> {
        let mut e = self.spawn((bundle, InGame));
        if ctx.replicates() {
            e.insert(Replicated);
        }
        e
    }

    fn spawn_projectile(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_> {
        let mut e = self.spawn((bundle, InGame));
        if ctx.replicates() {
            e.insert(Replicated);
        }
        e
    }

    fn spawn_impact(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_> {
        let mut e = self.spawn(bundle);
        if ctx.replicates() {
            e.insert(Replicated);
        }
        e
    }

    fn spawn_pickup(&mut self, ctx: SpawnContext, bundle: impl Bundle) -> EntityCommands<'_> {
        let mut e = self.spawn((bundle, InGame));
        if ctx.replicates() {
            e.insert(Replicated);
        }
        e
    }

    fn spawn_ingame(&mut self, bundle: impl Bundle) -> EntityCommands<'_> {
        self.spawn((bundle, InGame))
    }
}
