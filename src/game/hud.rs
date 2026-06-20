//! Power-up HUD (client-only).
//!
//! A bottom-centre tray showing the *local* player's active power-ups: each
//! shows the pickup's glowing glyph, a depleting bar, and a numeric countdown.
//!
//! Buffs themselves are authoritative-only, so the tray reads the small
//! replicated [`ActiveBuffs`](super::combat::ActiveBuffs) summary the server (or,
//! offline, the local sim) maintains. It reuses the pickup glyph art and per-kind
//! colours so a buff icon matches the orb the player walked over.
//!
//! Like the ping readout the root is spawned once at startup and persists across
//! lobby/round cleanups (it carries neither `InGame` nor `LobbyUi`). Six fixed
//! slots are pre-spawned and toggled via `Display`, so the per-frame update never
//! spawns or despawns anything.

use bevy::prelude::*;

use super::combat::{ActiveBuffs, BuffStatus};
use super::net::{NetRole, Predicted, is_client};
use super::pickup::{PickupArt, PickupKind, pickup_glow};
use super::player::Player;
use super::state::GameState;

/// Number of pre-spawned tray slots (one per timed buff kind).
const SLOT_COUNT: usize = 6;
/// Side length of a glyph icon, in logical pixels.
const GLYPH_PX: f32 = 44.0;
/// Width / height of the depleting countdown bar.
const BAR_W: f32 = 44.0;
const BAR_H: f32 = 5.0;
/// Empty-bar (track) colour behind the depleting fill.
const BAR_TRACK: Color = Color::srgba(0.08, 0.08, 0.11, 0.85);
/// Countdown text colour.
const TEXT_COLOR: Color = Color::srgb(0.85, 0.85, 0.92);

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_buff_hud.run_if(is_client))
            .add_systems(Update, update_buff_hud.run_if(is_client));
    }
}

/// Marks one tray slot (a column holding a glyph, bar and countdown). The index
/// selects which [`BuffStatus`] the slot mirrors.
#[derive(Component)]
struct BuffSlot(usize);

/// The glyph icon within slot `i`.
#[derive(Component)]
struct BuffGlyph(usize);

/// The depleting bar fill within slot `i`.
#[derive(Component)]
struct BuffBarFill(usize);

/// The numeric countdown text within slot `i`.
#[derive(Component)]
struct BuffCountdown(usize);

/// Spawns the persistent tray root plus its six hidden slots. Glyph images start
/// as a placeholder and are filled by [`update_buff_hud`], dodging any `Startup`
/// ordering question against the pickup-art builder.
fn spawn_buff_hud(mut commands: Commands) {
    commands
        .spawn((Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(16.0),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::FlexEnd,
            column_gap: Val::Px(12.0),
            ..default()
        },))
        .with_children(|root| {
            for i in 0..SLOT_COUNT {
                root.spawn((
                    BuffSlot(i),
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        row_gap: Val::Px(3.0),
                        // Hidden until the local player actually holds buff `i`.
                        display: Display::None,
                        ..default()
                    },
                ))
                .with_children(|slot| {
                    slot.spawn((
                        BuffGlyph(i),
                        ImageNode::new(Handle::default()),
                        Node {
                            width: Val::Px(GLYPH_PX),
                            height: Val::Px(GLYPH_PX),
                            ..default()
                        },
                    ));
                    slot.spawn((
                        Node {
                            width: Val::Px(BAR_W),
                            height: Val::Px(BAR_H),
                            ..default()
                        },
                        BackgroundColor(BAR_TRACK),
                    ))
                    .with_children(|track| {
                        track.spawn((
                            BuffBarFill(i),
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..default()
                            },
                            BackgroundColor(TEXT_COLOR),
                        ));
                    });
                    slot.spawn((
                        BuffCountdown(i),
                        Text::new(""),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(TEXT_COLOR),
                    ));
                });
            }
        });
}

/// Reads the local player's [`ActiveBuffs`] and drives the tray: shows one slot
/// per active buff with its glyph, a bar depleting with the remaining time, and a
/// seconds countdown; hides the rest. Every write is change-guarded to avoid
/// needless UI relayout (mirrors the ping/lobby idiom).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_buff_hud(
    state: Res<State<GameState>>,
    role: Res<NetRole>,
    art: Option<Res<PickupArt>>,
    local: Query<(&ActiveBuffs, Has<Predicted>), With<Player>>,
    mut slots: Query<(&BuffSlot, &mut Node), (Without<BuffBarFill>, Without<BuffGlyph>)>,
    mut glyphs: Query<(&BuffGlyph, &mut ImageNode)>,
    mut bars: Query<(&BuffBarFill, &mut Node, &mut BackgroundColor), Without<BuffSlot>>,
    mut texts: Query<(&BuffCountdown, &mut Text)>,
) {
    // The buffs to show: nothing outside the live round; online the controlled
    // (`Predicted`) player; offline the sole local `Player`. `ActiveBuffs` only
    // exists while a buff is active, so an empty/missing result hides the tray.
    let buffs: &[BuffStatus] = if *state.get() != GameState::Playing {
        &[]
    } else if matches!(*role, NetRole::OnlineClient) {
        local
            .iter()
            .find(|(_, predicted)| *predicted)
            .map(|(b, _)| b.0.as_slice())
            .unwrap_or(&[])
    } else {
        local
            .iter()
            .next()
            .map(|(b, _)| b.0.as_slice())
            .unwrap_or(&[])
    };

    for (slot, mut node) in &mut slots {
        let display = if slot.0 < buffs.len() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }

    let Some(art) = art else {
        return;
    };

    for (glyph, mut image) in &mut glyphs {
        if let Some(status) = buffs.get(glyph.0) {
            let handle = art.glyphs[status.kind.glyph_index()].clone();
            if image.image != handle {
                image.image = handle;
            }
            let tint = hud_tint(status.kind);
            if image.color != tint {
                image.color = tint;
            }
        }
    }

    for (bar, mut node, mut bg) in &mut bars {
        if let Some(status) = buffs.get(bar.0) {
            let frac = (status.remaining / status.total.max(1e-3)).clamp(0.0, 1.0);
            let width = Val::Percent(frac * 100.0);
            if node.width != width {
                node.width = width;
            }
            let tint = hud_tint(status.kind);
            if bg.0 != tint {
                bg.0 = tint;
            }
        }
    }

    for (countdown, mut text) in &mut texts {
        if let Some(status) = buffs.get(countdown.0) {
            let label = format!("{:.1}s", status.remaining.max(0.0));
            if text.0 != label {
                text.0 = label;
            }
        }
    }
}

/// The pickup glow colour, normalised to a vivid display-range tint. The world
/// orbs use HDR (> 1.0) colours so they bloom; UI isn't bloomed, so we scale the
/// brightest channel to 1.0 to keep each power-up's hue without washing to white.
fn hud_tint(kind: PickupKind) -> Color {
    let c = pickup_glow(kind).to_linear();
    let m = c.red.max(c.green).max(c.blue).max(1e-3);
    Color::linear_rgb(c.red / m, c.green / m, c.blue / m)
}
