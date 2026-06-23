//! Per-map background music.
//!
//! The [`Song`] enum is shared: the map parser (which runs on the server too)
//! needs it to read the `song:` directive. Actually *playing* audio is
//! client-only, so the plugin and its system are gated on the `client` feature.

#[cfg(feature = "client")]
use super::net::SpawnCommandsExt;
#[cfg(feature = "client")]
use bevy::prelude::*;

/// A background track, chosen per-map. Variants map to files in `assets/music/`
/// via [`Song::asset_path`], mirroring the `PlayerColor::asset_path` pattern.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Song {
    #[default]
    ShooterLoop,
    Funky,
    Playful,
    Rocky,
    Sinister,
}

impl Song {
    /// Asset path relative to `assets/`.
    pub fn asset_path(self) -> &'static str {
        match self {
            Song::ShooterLoop => "music/shooter_loop.mp3",
            Song::Funky => "music/song_funky.mp3",
            Song::Playful => "music/song_playful.mp3",
            Song::Rocky => "music/song_rocky.mp3",
            Song::Sinister => "music/song_sinister.mp3",
        }
    }

    /// Parses a map-file `song:` value. Case-insensitive; returns `None` for
    /// unknown names so the caller can warn and fall back to the default.
    pub fn from_name(name: &str) -> Option<Song> {
        match name.trim().to_ascii_lowercase().as_str() {
            "shooter_loop" | "shooter" => Some(Song::ShooterLoop),
            "funky" => Some(Song::Funky),
            "playful" => Some(Song::Playful),
            "rocky" => Some(Song::Rocky),
            "sinister" => Some(Song::Sinister),
            _ => None,
        }
    }
}

/// Marker for the looping background-music entity.
#[cfg(feature = "client")]
#[derive(Component)]
pub struct BackgroundMusic;

#[cfg(feature = "client")]
pub struct MusicPlugin;

#[cfg(feature = "client")]
impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(super::state::GameState::Playing), start_music);
    }
}

#[cfg(feature = "client")]
fn start_music(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    map: Res<super::map::CurrentMap>,
) {
    let song = map.0.song();
    commands.spawn_ingame((
        BackgroundMusic,
        AudioPlayer::new(asset_server.load(song.asset_path())),
        PlaybackSettings {
            mode: bevy::audio::PlaybackMode::Loop,
            volume: bevy::audio::Volume::Linear(0.5),
            ..default()
        },
    ));
}
