//! Ping readout (client-only).
//!
//! Shows the connection's round-trip time to the server in a corner of the
//! screen so an online player can see their latency. Only meaningful for
//! [`ClientBackend`](super::net::ClientBackend): offline has no server and the
//! dedicated server never renders, so the HUD is spawned (and the systems run)
//! only for the online-client backend.
//!
//! The readout is spawned once at startup and persists across the lobby→match
//! transition: it carries neither `InGame` nor `LobbyUi`, so the state-exit
//! cleanups (`cleanup_ingame` / `despawn_lobby`) leave it untouched, exactly like
//! the replicated `MatchInfo` singleton survives the whole session.

use std::marker::PhantomData;

use bevy::prelude::*;
use bevy_replicon_renet::RenetClient;

use super::net::NetworkBackend;

pub struct PingPlugin<B: NetworkBackend> {
    _backend: PhantomData<B>,
}

impl<B: NetworkBackend> PingPlugin<B> {
    pub fn new() -> Self {
        Self {
            _backend: PhantomData,
        }
    }
}

impl<B: NetworkBackend> Plugin for PingPlugin<B> {
    fn build(&self, app: &mut App) {
        if B::IS_ONLINE_CLIENT {
            app.add_systems(Startup, spawn_ping_hud)
                .add_systems(Update, update_ping_hud);
        }
    }
}

/// Marks the on-screen ping text node so the update system can find it.
#[derive(Component)]
struct PingText;

/// Spawns the persistent ping readout in the top-left corner. Absolutely
/// positioned so it floats over whatever screen (lobby or match) is showing.
fn spawn_ping_hud(mut commands: Commands) {
    commands.spawn((
        PingText,
        Text::new("Ping: -- ms"),
        TextFont {
            font_size: 18.0,
            ..default()
        },
        TextColor(NO_PING),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(8.0),
            ..default()
        },
    ));
}

/// Refreshes the readout from renet's RTT estimate (seconds → ms). renet already
/// smooths the RTT with an EMA, so we just convert and round. The label is only
/// rewritten when its text or colour actually changes, to avoid needless UI
/// re-layout every frame (mirrors the lobby's label idiom). `RenetClient` is
/// taken as an `Option` because it only exists once the client transport is set
/// up; before connecting (or between acks) renet reports no RTT yet.
fn update_ping_hud(
    client: Option<Res<RenetClient>>,
    mut query: Query<(&mut Text, &mut TextColor), With<PingText>>,
) {
    let Ok((mut text, mut color)) = query.single_mut() else {
        return;
    };
    let (label, next_color) = match client.as_deref() {
        Some(client) if client.is_connected() => {
            let ms = (client.rtt() * 1000.0).round() as u32;
            (format!("Ping: {ms} ms"), ping_color(ms))
        }
        _ => ("Ping: -- ms".to_string(), NO_PING),
    };
    if text.0 != label {
        text.0 = label;
    }
    if color.0 != next_color {
        color.0 = next_color;
    }
}

/// Colour shown before a connection/RTT estimate exists.
const NO_PING: Color = Color::srgb(0.6, 0.6, 0.6);

/// Traffic-light colouring of the latency so quality reads at a glance.
fn ping_color(ms: u32) -> Color {
    if ms < 60 {
        Color::srgb(0.5, 0.9, 0.5) // good
    } else if ms < 120 {
        Color::srgb(0.9, 0.85, 0.4) // ok
    } else {
        Color::srgb(0.9, 0.45, 0.45) // laggy
    }
}
