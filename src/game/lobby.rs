//! Pre-match lobby (client-only).
//!
//! Shown while in [`GameState::Lobby`]. The game's owner — the first client to
//! join online, or the local player offline — picks the map and bot count and
//! presses Start; everyone else waits. Offline the owner starts the match
//! locally; online the owner sends a [`StartMatch`] event and the whole lobby
//! transitions to [`GameState::Playing`] when the server's [`MatchInfo`] arrives.
//!
//! The UI is deliberately button-only (no text entry): the join code is supplied
//! on the command line / `JOIN_CODE` env var, not typed here.

use bevy::prelude::*;
use bevy_replicon::prelude::*;

use super::map::{self, MAPS};
use super::net::{MatchInfo, MatchPhase, NetRole, StartMatch, Winner, YouAreOwner, is_offline};
use super::state::{GameState, MatchConfig};

/// Largest bot count the owner can dial up to in the lobby.
const MAX_BOTS: u8 = 16;

pub struct LobbyPlugin;

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<IsOwner>()
            .init_resource::<LobbyDraft>()
            .add_systems(
                OnEnter(GameState::Lobby),
                (spawn_lobby_camera, spawn_lobby_ui),
            )
            .add_systems(
                OnEnter(GameState::Lobby),
                set_offline_owner.run_if(is_offline),
            )
            .add_systems(OnExit(GameState::Lobby), despawn_lobby)
            .add_systems(
                Update,
                (handle_buttons, update_labels, update_visibility)
                    .run_if(in_state(GameState::Lobby)),
            )
            // The owner client learns it's the owner (no-op in other modes).
            // Online clients follow the match lifecycle via `match_flow`.
            .add_observer(on_you_are_owner);
    }
}

/// True once this client knows it owns the game (set offline on entering the
/// lobby, online when the server's [`YouAreOwner`] event arrives).
#[derive(Resource, Default)]
struct IsOwner(bool);

/// The owner's in-progress selections, edited by the lobby buttons. Copied into
/// the authoritative [`MatchConfig`] only when the match starts.
#[derive(Resource)]
struct LobbyDraft {
    map_index: u8,
    bot_count: u8,
}

impl Default for LobbyDraft {
    fn default() -> Self {
        let config = MatchConfig::default();
        Self {
            map_index: config.map_index,
            bot_count: config.bot_count,
        }
    }
}

/// Marks the lobby's camera and UI root so they can be cleared on exit.
#[derive(Component)]
struct LobbyUi;

/// Tags a clickable lobby button with the action it performs.
#[derive(Component, Clone, Copy)]
enum LobbyButton {
    MapPrev,
    MapNext,
    BotMinus,
    BotPlus,
    Start,
}

/// Text node showing the selected map's name.
#[derive(Component)]
struct MapNameLabel;
/// Text node showing the selected bot count.
#[derive(Component)]
struct BotCountLabel;

/// A top-level lobby section that is shown or hidden depending on connection and
/// ownership state. Tagging all three with one component lets a single system
/// toggle their visibility.
#[derive(Component, Clone, Copy, PartialEq)]
enum LobbySection {
    /// "Connecting…" — online, before the server connection is established.
    Connecting,
    /// The owner's map/bot configuration controls.
    Config,
    /// "Waiting for host…" — connected non-owners.
    Waiting,
}

fn set_offline_owner(mut is_owner: ResMut<IsOwner>) {
    is_owner.0 = true;
}

/// The lobby needs its own camera since the gameplay camera isn't spawned until
/// `Playing`. Tagged `LobbyUi` so it's despawned when the lobby closes.
fn spawn_lobby_camera(mut commands: Commands) {
    commands.spawn((Camera2d, LobbyUi));
}

fn spawn_lobby_ui(mut commands: Commands) {
    commands
        .spawn((
            LobbyUi,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(28.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.05, 0.05, 0.08)),
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("SUPER BATTLE ROYALE"),
                TextFont {
                    font_size: 48.0,
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.9, 1.0)),
            ));

            root.spawn((
                LobbySection::Connecting,
                Text::new("Connecting to server..."),
                TextFont {
                    font_size: 26.0,
                    ..default()
                },
                TextColor(Color::srgb(0.8, 0.8, 0.6)),
            ));

            root.spawn((
                LobbySection::Waiting,
                Text::new("Waiting for the host to start..."),
                TextFont {
                    font_size: 26.0,
                    ..default()
                },
                TextColor(Color::srgb(0.8, 0.8, 0.6)),
            ));

            root.spawn((
                LobbySection::Config,
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(20.0),
                    ..default()
                },
            ))
            .with_children(|panel| {
                // Map selector: [<]  MapName  [>]
                panel
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(16.0),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_button(row, LobbyButton::MapPrev, "<");
                        row.spawn((
                            MapNameLabel,
                            Text::new(""),
                            TextFont {
                                font_size: 32.0,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                            Node {
                                width: Val::Px(220.0),
                                justify_content: JustifyContent::Center,
                                ..default()
                            },
                        ));
                        spawn_button(row, LobbyButton::MapNext, ">");
                    });

                // Bot count selector: Bots:  [-]  N  [+]
                panel
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(16.0),
                        ..default()
                    })
                    .with_children(|row| {
                        row.spawn((
                            Text::new("Bots:"),
                            TextFont {
                                font_size: 32.0,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        ));
                        spawn_button(row, LobbyButton::BotMinus, "-");
                        row.spawn((
                            BotCountLabel,
                            Text::new(""),
                            TextFont {
                                font_size: 32.0,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                            Node {
                                width: Val::Px(48.0),
                                justify_content: JustifyContent::Center,
                                ..default()
                            },
                        ));
                        spawn_button(row, LobbyButton::BotPlus, "+");
                    });

                spawn_button(panel, LobbyButton::Start, "START");
            });
        });
}

/// Spawns one labelled button as a child of `parent`.
fn spawn_button(parent: &mut ChildSpawnerCommands, kind: LobbyButton, label: &str) {
    parent
        .spawn((
            Button,
            kind,
            Node {
                padding: UiRect::axes(Val::Px(22.0), Val::Px(12.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgb(0.2, 0.2, 0.32)),
        ))
        .with_children(|button| {
            button.spawn((
                Text::new(label),
                TextFont {
                    font_size: 30.0,
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
}

fn despawn_lobby(mut commands: Commands, query: Query<Entity, With<LobbyUi>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

/// Applies button clicks: cycles the map, adjusts the bot count, or starts the
/// match. Starting is gated on ownership here (the server validates it too).
fn handle_buttons(
    interactions: Query<(&Interaction, &LobbyButton), Changed<Interaction>>,
    mut draft: ResMut<LobbyDraft>,
    is_owner: Res<IsOwner>,
    role: Res<NetRole>,
    mut commands: Commands,
    mut config: ResMut<MatchConfig>,
    mut next: ResMut<NextState<GameState>>,
) {
    let map_count = MAPS.len() as u8;
    for (interaction, button) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            LobbyButton::MapPrev => {
                draft.map_index = (draft.map_index + map_count - 1) % map_count;
            }
            LobbyButton::MapNext => {
                draft.map_index = (draft.map_index + 1) % map_count;
            }
            LobbyButton::BotMinus => {
                draft.bot_count = draft.bot_count.saturating_sub(1);
            }
            LobbyButton::BotPlus => {
                draft.bot_count = (draft.bot_count + 1).min(MAX_BOTS);
            }
            LobbyButton::Start => {
                if !is_owner.0 {
                    continue;
                }
                match *role {
                    NetRole::Offline => {
                        // Apply the config and start locally. Insert the map
                        // resources *before* the state change: the OnEnter(Playing)
                        // spawn systems read them.
                        config.map_index = draft.map_index;
                        config.bot_count = draft.bot_count;
                        map::insert_map_resources(&mut commands, draft.map_index);
                        // The match-state singleton (local, not replicated offline)
                        // that `match_flow` drives and the winner banner reads.
                        commands.spawn(MatchInfo {
                            map_index: draft.map_index,
                            round: 0,
                            phase: MatchPhase::Playing,
                            winner: Winner::Draw,
                        });
                        next.set(GameState::Playing);
                    }
                    NetRole::OnlineClient => {
                        // Ask the server to start; we transition when MatchInfo arrives.
                        commands.client_trigger(StartMatch {
                            map_index: draft.map_index,
                            bot_count: draft.bot_count,
                        });
                    }
                    NetRole::Server => {}
                }
            }
        }
    }
}

/// Keeps the map-name and bot-count labels in sync with the draft selection.
fn update_labels(
    draft: Res<LobbyDraft>,
    mut map_label: Query<&mut Text, (With<MapNameLabel>, Without<BotCountLabel>)>,
    mut bot_label: Query<&mut Text, (With<BotCountLabel>, Without<MapNameLabel>)>,
) {
    if let Ok(mut text) = map_label.single_mut() {
        let name = MAPS[draft.map_index as usize % MAPS.len()].0;
        if text.0 != name {
            text.0 = name.to_string();
        }
    }
    if let Ok(mut text) = bot_label.single_mut() {
        let count = draft.bot_count.to_string();
        if text.0 != count {
            text.0 = count;
        }
    }
}

/// Shows exactly one section: "connecting" (online, not yet connected), the
/// owner's config panel (owner), or "waiting for host" (connected non-owner).
fn update_visibility(
    role: Res<NetRole>,
    is_owner: Res<IsOwner>,
    client_state: Option<Res<State<ClientState>>>,
    mut sections: Query<(&mut Node, &LobbySection)>,
) {
    // Offline is always "connected"; online depends on the replicon client state.
    let connected = match *role {
        NetRole::Offline => true,
        _ => client_state
            .map(|s| *s.get() == ClientState::Connected)
            .unwrap_or(false),
    };

    for (mut node, section) in &mut sections {
        let visible = match section {
            LobbySection::Connecting => *role == NetRole::OnlineClient && !connected,
            LobbySection::Config => connected && is_owner.0,
            LobbySection::Waiting => connected && !is_owner.0,
        };
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
}

fn on_you_are_owner(_event: On<YouAreOwner>, mut is_owner: ResMut<IsOwner>) {
    is_owner.0 = true;
    info!("you are the game owner");
}
