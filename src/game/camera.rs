use bevy::camera::ScalingMode;
use bevy::prelude::*;

use super::InGame;
use super::arena::ARENA_HEIGHT;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(super::state::AppState::InGame), spawn_camera);
    }
}

fn spawn_camera(mut commands: Commands) {
    // Fixed camera that shows the entire arena regardless of window size.
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: ARENA_HEIGHT + 80.0,
            },
            ..OrthographicProjection::default_2d()
        }),
        Transform::from_xyz(0.0, 0.0, 1000.0),
        InGame,
    ));
}
