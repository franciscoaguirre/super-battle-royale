use bevy::camera::ScalingMode;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::{Bloom, BloomCompositeMode, BloomPrefilter};
use bevy::prelude::*;

use super::InGame;
use super::map::ArenaBounds;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(super::state::GameState::Playing), spawn_camera);
    }
}

fn spawn_camera(mut commands: Commands, bounds: Res<ArenaBounds>) {
    // Fixed camera that shows the entire arena (plus a small margin) regardless
    // of window size.
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: bounds.size().y + 80.0,
            },
            ..OrthographicProjection::default_2d()
        }),
        Transform::from_xyz(0.0, 0.0, 1000.0),
        // Bloom makes the HDR-bright projectiles (and their trails) glow. It
        // requires `Hdr`, which it pulls in automatically. The prefilter
        // threshold means only pixels brighter than 1.0 bloom, so the regular
        // pixel-art scene is left untouched.
        Bloom {
            intensity: 0.3,
            prefilter: BloomPrefilter {
                threshold: 1.0,
                threshold_softness: 0.4,
            },
            composite_mode: BloomCompositeMode::Additive,
            ..Bloom::NATURAL
        },
        // Skip tonemapping so the SDR sprite art keeps its authored colors; the
        // additive bloom is layered on top.
        Tonemapping::None,
        InGame,
    ));
}
