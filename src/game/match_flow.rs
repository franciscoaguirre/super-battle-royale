//! Round lifecycle: permadeath ends a round when one combatant is left standing,
//! the winner is announced over a brief frozen pause, then the next map loads and
//! a fresh round begins — looping.
//!
//! The authoritative side (offline + server) detects the winner and drives the
//! flow with `GameState` (which freezes the sim during `GameOver`) and the
//! replicated [`MatchInfo`] singleton. Online clients run no game logic here —
//! they mirror their local `GameState` from `MatchInfo` ([`follow_match_phase`]).
//! "Last one standing" counts every combatant (players *and* bots), so the
//! survivor may be a player, a bot, or a draw if the last two die together.

use bevy::prelude::*;

use super::bot::Bot;
use super::combat::{Dead, Health};
use super::map::{self, MAPS};
use super::net::{MatchInfo, MatchPhase, Winner, is_authoritative, is_online_client};
use super::player::{Player, PlayerColor};
use super::state::{GameState, MatchConfig};

/// Seconds the winner is announced before the next map loads.
const ANNOUNCE_SECS: f32 = 4.0;

/// Authoritative-only countdown during the `GameOver` announcement.
#[derive(Resource)]
struct IntermissionTimer(Timer);

pub struct MatchFlowPlugin;

impl Plugin for MatchFlowPlugin {
    fn build(&self, app: &mut App) {
        // Authoritative: detect the winner, run the announcement, advance maps.
        app.add_systems(
            Update,
            check_for_winner
                .run_if(in_state(GameState::Playing))
                .run_if(is_authoritative),
        )
        .add_systems(
            OnEnter(GameState::GameOver),
            start_intermission.run_if(is_authoritative),
        )
        .add_systems(
            Update,
            advance_after_intermission
                .run_if(in_state(GameState::GameOver))
                .run_if(is_authoritative),
        );

        // Online clients mirror their local state from the replicated MatchInfo.
        app.add_systems(Update, follow_match_phase.run_if(is_online_client));

        // Client: announce the winner over the frozen scene during GameOver.
        #[cfg(feature = "client")]
        app.add_systems(OnEnter(GameState::GameOver), spawn_winner_banner)
            .add_systems(OnExit(GameState::GameOver), despawn_winner_banner);
    }
}

/// Ends the round once at most one combatant is left alive (and at least one has
/// died — guards the spawn frame and never-ending 1-combatant matches). Records
/// the winner in [`MatchInfo`] (replicated to clients) and transitions to
/// `GameOver`.
#[allow(clippy::type_complexity)]
fn check_for_winner(
    mut info: Query<&mut MatchInfo>,
    mut next: ResMut<NextState<GameState>>,
    alive: Query<
        (Has<Player>, &PlayerColor),
        (Or<(With<Player>, With<Bot>)>, With<Health>, Without<Dead>),
    >,
    dead: Query<(), (Or<(With<Player>, With<Bot>)>, With<Dead>)>,
) {
    let Ok(mut info) = info.single_mut() else {
        return;
    };
    if info.phase != MatchPhase::Playing {
        return;
    }

    let alive_count = alive.iter().count();
    if alive_count > 1 || dead.is_empty() {
        return; // still contested, or no death yet (spawn frame / lone combatant)
    }

    info.winner = match alive.iter().next() {
        Some((true, color)) => Winner::Player(*color),
        Some((false, _)) => Winner::Bot,
        None => Winner::Draw,
    };
    info.phase = MatchPhase::Ended;
    next.set(GameState::GameOver);
}

/// Begins the announcement countdown when a round ends.
fn start_intermission(mut commands: Commands) {
    commands.insert_resource(IntermissionTimer(Timer::from_seconds(
        ANNOUNCE_SECS,
        TimerMode::Once,
    )));
}

/// After the announcement, advances to the next map (wrapping) and starts a fresh
/// round: loads the map, bumps [`MatchInfo`] (new round, `Playing`), and
/// transitions back to `Playing`. Inserting the map resources before the state
/// change is required — the `OnEnter(Playing)` spawn systems read them.
fn advance_after_intermission(
    time: Res<Time>,
    mut commands: Commands,
    timer: Option<ResMut<IntermissionTimer>>,
    mut config: ResMut<MatchConfig>,
    mut info: Query<&mut MatchInfo>,
    mut next: ResMut<NextState<GameState>>,
) {
    let Some(mut timer) = timer else {
        return;
    };
    if !timer.0.tick(time.delta()).just_finished() {
        return;
    }

    let next_index = ((config.map_index as usize + 1) % MAPS.len()) as u8;
    config.map_index = next_index;
    map::insert_map_resources(&mut commands, next_index);
    if let Ok(mut info) = info.single_mut() {
        info.map_index = next_index;
        info.round = info.round.wrapping_add(1);
        info.phase = MatchPhase::Playing;
        info.winner = Winner::Draw;
    }
    next.set(GameState::Playing);
}

/// Online clients mirror their local `GameState` from the replicated `MatchInfo`:
/// `Ended` → show the announcement (`GameOver`); a new `round` in `Playing` →
/// load that map and (re)enter `Playing`. Generalizes the old "match started"
/// observer to cover the first round and every map switch.
fn follow_match_phase(
    info: Query<&MatchInfo>,
    state: Res<State<GameState>>,
    mut next: ResMut<NextState<GameState>>,
    mut last_round: Local<Option<u32>>,
    mut commands: Commands,
    mut config: ResMut<MatchConfig>,
) {
    let Ok(info) = info.single() else {
        return;
    };
    match info.phase {
        MatchPhase::Ended => {
            if *state.get() != GameState::GameOver {
                next.set(GameState::GameOver);
            }
        }
        MatchPhase::Playing => {
            if *last_round != Some(info.round) {
                config.map_index = info.map_index;
                map::insert_map_resources(&mut commands, info.map_index);
                next.set(GameState::Playing);
                *last_round = Some(info.round);
            }
        }
    }
}

/// Client-only marker for the winner-announcement UI root.
#[cfg(feature = "client")]
#[derive(Component)]
struct WinnerBanner;

/// Spawns the centered "X wins!" overlay over the frozen scene.
#[cfg(feature = "client")]
fn spawn_winner_banner(mut commands: Commands, info: Query<&MatchInfo>) {
    let text = match info.single().map(|i| i.winner) {
        Ok(Winner::Player(color)) => format!("{color:?} wins!"),
        Ok(Winner::Bot) => "A bot wins!".to_string(),
        _ => "Draw!".to_string(),
    };
    commands
        .spawn((
            WinnerBanner,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Text::new(text),
                TextFont {
                    font_size: 64.0,
                    ..default()
                },
                TextColor(Color::srgb(1.0, 1.0, 0.7)),
            ));
        });
}

#[cfg(feature = "client")]
fn despawn_winner_banner(mut commands: Commands, banners: Query<Entity, With<WinnerBanner>>) {
    for entity in &banners {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    // `super::*` re-exports the types match_flow imports (Player, PlayerColor,
    // Health, MatchInfo, MatchPhase, Winner, MatchConfig, GameState, MAPS, ...).
    use super::*;
    use crate::game::combat::CombatPlugin;
    use crate::game::map::{CurrentMap, TileMap};
    use crate::game::net::NetPos;
    use crate::game::net::NetRole;
    use crate::game::shield::{ShieldCharge, ShieldPlugin, ShieldState, ShieldStatus, Shielding};
    use crate::game::state::MatchConfig;
    use bevy::state::app::StatesPlugin;
    use std::time::Duration;

    /// A headless authoritative app running just combat + match-flow (no bot/player
    /// spawn plugins), so the only combatants are the ones the test spawns.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            CombatPlugin,
            ShieldPlugin,
            MatchFlowPlugin,
        ));
        app.insert_resource(NetRole::Offline);
        app.insert_resource(CurrentMap(TileMap::parse("wsw")));
        app.init_resource::<MatchConfig>();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f32(1.0 / 60.0),
        ));
        app.insert_state(GameState::Playing);
        app.world_mut().spawn(MatchInfo::default()); // round 0, Playing, Draw, map 0
        app
    }

    fn current_state(app: &App) -> GameState {
        *app.world().resource::<State<GameState>>().get()
    }

    fn info(app: &mut App) -> MatchInfo {
        *app.world_mut()
            .query::<&MatchInfo>()
            .single(app.world())
            .unwrap()
    }

    #[test]
    fn last_one_standing_ends_the_round_with_that_winner() {
        let mut app = test_app();
        app.world_mut().spawn((Player, PlayerColor::Blue));
        let red = app.world_mut().spawn((Player, PlayerColor::Red)).id();
        app.update(); // both get full health; nobody dead yet
        assert_eq!(current_state(&app), GameState::Playing);

        app.world_mut().get_mut::<Health>(red).unwrap().current = 0.0;
        for _ in 0..4 {
            app.update(); // death → win detected → transition
        }
        assert_eq!(current_state(&app), GameState::GameOver);
        let info = info(&mut app);
        assert_eq!(info.phase, MatchPhase::Ended);
        assert_eq!(info.winner, Winner::Player(PlayerColor::Blue));
    }

    #[test]
    fn simultaneous_final_deaths_are_a_draw() {
        let mut app = test_app();
        let a = app.world_mut().spawn((Player, PlayerColor::Blue)).id();
        let b = app.world_mut().spawn((Player, PlayerColor::Red)).id();
        app.update();
        app.world_mut().get_mut::<Health>(a).unwrap().current = 0.0;
        app.world_mut().get_mut::<Health>(b).unwrap().current = 0.0;
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(current_state(&app), GameState::GameOver);
        assert_eq!(info(&mut app).winner, Winner::Draw);
    }

    #[test]
    fn round_does_not_end_before_a_death() {
        let mut app = test_app();
        app.world_mut().spawn((Player, PlayerColor::Blue));
        app.world_mut().spawn((Player, PlayerColor::Red));
        for _ in 0..5 {
            app.update();
        }
        // Two alive, none dead → no premature win (guards the spawn frame).
        assert_eq!(current_state(&app), GameState::Playing);
    }

    #[test]
    fn intermission_advances_to_the_next_map() {
        let mut app = test_app();
        app.world_mut().spawn((Player, PlayerColor::Blue));
        let red = app.world_mut().spawn((Player, PlayerColor::Red)).id();
        app.update();
        app.world_mut().get_mut::<Health>(red).unwrap().current = 0.0;
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(current_state(&app), GameState::GameOver);
        assert_eq!(info(&mut app).map_index, 0);

        // Wait out the announcement (~4 s = 240 ticks) plus margin.
        for _ in 0..300 {
            app.update();
        }
        assert_eq!(current_state(&app), GameState::Playing);
        let info = info(&mut app);
        assert_eq!(info.map_index, 1 % MAPS.len() as u8);
        assert_eq!(info.round, 1);
        assert_eq!(info.phase, MatchPhase::Playing);
    }

    /// Regression: the shield must be fully reset at the start of a new round,
    /// even for the surviving winner that carried an active shield into the
    /// intermission. Without the reset, the absolute-time active timer expires
    /// during `GameOver` and leaves the player on cooldown.
    #[test]
    fn shield_resets_between_rounds() {
        let mut app = test_app();
        let winner = app
            .world_mut()
            .spawn((
                Player,
                PlayerColor::Blue,
                NetPos(Vec2::ZERO),
                ShieldState::default(),
            ))
            .id();
        let loser = app
            .world_mut()
            .spawn((
                Player,
                PlayerColor::Red,
                NetPos(Vec2::ZERO),
                ShieldState::default(),
            ))
            .id();

        // First tick grants health.
        app.update();

        // Raise the winner's shield and let it tick for a few frames.
        app.world_mut()
            .get_mut::<ShieldState>(winner)
            .unwrap()
            .requested = true;
        for _ in 0..10 {
            app.update();
        }
        assert!(
            app.world().get::<Shielding>(winner).is_some(),
            "winner should be shielding before the round ends"
        );
        assert!(
            app.world().get::<ShieldCharge>(winner).unwrap().0 < 1.0,
            "shield charge should have drained while active"
        );

        // Kill the other player to end the round.
        app.world_mut().get_mut::<Health>(loser).unwrap().current = 0.0;
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(current_state(&app), GameState::GameOver);

        // Wait out the intermission and one extra frame for the state change.
        for _ in 0..310 {
            app.update();
        }
        assert_eq!(current_state(&app), GameState::Playing);

        let state = app.world().get::<ShieldState>(winner).unwrap();
        assert!(
            matches!(state.status, ShieldStatus::Ready),
            "winner's shield should be Ready after a new round starts"
        );
        assert_eq!(state.charge, 1.0, "winner's shield charge should be full");
        assert!(
            app.world().get::<Shielding>(winner).is_none(),
            "winner should not have the Shielding marker at round start"
        );
        assert_eq!(
            app.world().get::<ShieldCharge>(winner).unwrap().0,
            1.0,
            "replicated shield charge should be full"
        );
    }
}
