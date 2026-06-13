use bevy::prelude::*;

use super::InGame;

/// Marker for the looping background-music entity.
#[derive(Component)]
pub struct BackgroundMusic;

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(super::state::AppState::InGame), start_music);
    }
}

fn start_music(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn((
        BackgroundMusic,
        AudioPlayer::new(asset_server.load("music/shooter_loop.mp3")),
        PlaybackSettings {
            mode: bevy::audio::PlaybackMode::Loop,
            volume: bevy::audio::Volume::Linear(0.5),
            ..default()
        },
        InGame,
    ));
}
