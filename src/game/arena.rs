use bevy::prelude::*;

use super::InGame;

pub const ARENA_WIDTH: f32 = 800.0;
pub const ARENA_HEIGHT: f32 = 600.0;

pub struct ArenaPlugin;

impl Plugin for ArenaPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(super::state::GameState::Playing), spawn_arena);
    }
}

fn spawn_arena(mut commands: Commands) {
    // Grass background
    commands.spawn((
        Sprite {
            color: Color::srgb(0.15, 0.45, 0.15),
            custom_size: Some(Vec2::new(ARENA_WIDTH, ARENA_HEIGHT)),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 0.0),
        InGame,
    ));

    // Boundary walls
    let wall_thickness = 20.0;
    let half_w = ARENA_WIDTH / 2.0 + wall_thickness / 2.0;
    let half_h = ARENA_HEIGHT / 2.0 + wall_thickness / 2.0;
    let wall_color = Color::srgb(0.4, 0.25, 0.1);

    // Top and bottom
    for y in [-half_h, half_h] {
        commands.spawn((
            Sprite {
                color: wall_color,
                custom_size: Some(Vec2::new(
                    ARENA_WIDTH + wall_thickness * 2.0,
                    wall_thickness,
                )),
                ..default()
            },
            Transform::from_xyz(0.0, y, 0.5),
            InGame,
        ));
    }

    // Left and right
    for x in [-half_w, half_w] {
        commands.spawn((
            Sprite {
                color: wall_color,
                custom_size: Some(Vec2::new(wall_thickness, ARENA_HEIGHT)),
                ..default()
            },
            Transform::from_xyz(x, 0.0, 0.5),
            InGame,
        ));
    }
}
