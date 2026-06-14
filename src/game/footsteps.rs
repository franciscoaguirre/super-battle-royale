use bevy::prelude::*;

use super::InGame;
use super::player::Player;

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

/// Footsteps play quieter than the background music so they sit underneath it.
const STEP_VOLUME: f32 = 0.6;

/// Squared distance the player must travel in a frame to count as "walking",
/// which filters out floating-point jitter when standing still.
const MOVE_EPSILON_SQ: f32 = 0.01 * 0.01;

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
/// PRNG for clip selection, the previous player position (to detect movement),
/// and the last clip played (to avoid repeating it back-to-back).
#[derive(Resource)]
struct FootstepState {
    timer: Timer,
    rng: u64,
    last_pos: Option<Vec2>,
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
            last_pos: None,
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
    query: Query<&Transform, With<Player>>,
) {
    let Some(transform) = query.iter().next() else {
        return;
    };

    let pos = transform.translation.truncate();
    let moving = match state.last_pos {
        Some(prev) => prev.distance_squared(pos) > MOVE_EPSILON_SQ,
        None => false,
    };
    state.last_pos = Some(pos);

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
