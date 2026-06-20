use bevy::prelude::*;

use super::InGame;
use super::player::{Player, PlayerIntent};

/// The interchangeable footstep clips, relative to the `assets/` dir. One is
/// chosen at random each step so walking doesn't sound mechanically repetitive.
const STEP_PATHS: [&str; 4] = [
    "soundfx/sfx_step_1.mp3",
    "soundfx/sfx_step_2.mp3",
    "soundfx/sfx_step_3.mp3",
    "soundfx/sfx_step_4.mp3",
];

/// Seconds between footstep sounds while the player is moving.
const STEP_INTERVAL: f32 = 0.24;

/// Footsteps play quieter than the background music (which is `0.5`) so they sit
/// underneath it.
const STEP_VOLUME: f32 = 0.3;

pub struct FootstepsPlugin;

impl Plugin for FootstepsPlugin {
    fn build(&self, app: &mut App) {
        // Footsteps are about the *local* player, which only exists in offline
        // single-player; online clients render remote players whose footsteps we
        // don't simulate locally.
        app.insert_resource(FootstepState::new()).add_systems(
            Update,
            play_footsteps
                .run_if(in_state(super::state::GameState::Playing))
                .run_if(super::net::is_offline),
        );
    }
}

/// Per-frame bookkeeping for the footstep system: the cadence timer, a small
/// PRNG for clip selection, and the last clip played (to avoid repeating it
/// back-to-back).
#[derive(Resource)]
struct FootstepState {
    timer: Timer,
    rng: u64,
    last_index: usize,
}

impl FootstepState {
    fn new() -> Self {
        let mut timer = Timer::from_seconds(STEP_INTERVAL, TimerMode::Repeating);
        // Start "ready" so the first step lands as soon as the player moves.
        let duration = timer.duration();
        timer.set_elapsed(duration);
        Self {
            timer,
            rng: 0x9E37_79B9_7F4A_7C15,
            last_index: usize::MAX,
        }
    }

    /// Advances the xorshift64 state and returns the next pseudo-random value.
    fn next_rng(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// Picks a step clip index, never the same one twice in a row.
    fn pick_index(&mut self) -> usize {
        let index = (self.next_rng() % STEP_PATHS.len() as u64) as usize;
        let index = if index == self.last_index {
            (index + 1) % STEP_PATHS.len()
        } else {
            index
        };
        self.last_index = index;
        index
    }
}

fn play_footsteps(
    time: Res<Time>,
    mut state: ResMut<FootstepState>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    query: Query<&PlayerIntent, With<Player>>,
) {
    let Some(intent) = query.iter().next() else {
        return;
    };

    // Drive the cadence off the player's *input* rather than per-frame position
    // deltas: movement runs in `FixedUpdate` (60 Hz) while this system runs in
    // `Update` (render rate), so a faster display sees position-unchanged frames
    // between fixed steps. Detecting movement from those deltas would falsely go
    // idle on those frames and re-fire the moment the next step lands, producing
    // a rapid, overlapping stutter of footsteps. The intent is a clean per-frame
    // signal of "the player is walking".
    let moving = intent.0 != Vec2::ZERO;

    if !moving {
        // Reset the cadence and leave it ready to fire on the next move.
        state.timer.reset();
        let duration = state.timer.duration();
        state.timer.set_elapsed(duration);
        return;
    }

    state.timer.tick(time.delta());
    if !state.timer.just_finished() {
        return;
    }

    // Mix in real elapsed time so the clip order varies between runs.
    state.rng ^= time.elapsed().as_nanos() as u64;
    let index = state.pick_index();

    commands.spawn((
        AudioPlayer::new(asset_server.load(STEP_PATHS[index])),
        PlaybackSettings {
            mode: bevy::audio::PlaybackMode::Despawn,
            volume: bevy::audio::Volume::Linear(STEP_VOLUME),
            ..default()
        },
        InGame,
    ));
}
