use bevy::prelude::*;

use super::InGame;
use super::map::ArenaBounds;

pub const ENEMY_SIZE: f32 = 32.0;
const ENEMY_SPEED: f32 = 120.0;

#[derive(Component)]
pub struct Enemy {
    direction: Vec2,
}

pub struct EnemyPlugin;

impl Plugin for EnemyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(super::state::GameState::Playing), spawn_enemies)
            .add_systems(
                Update,
                patrol_enemies.run_if(in_state(super::state::GameState::Playing)),
            );
    }
}

fn spawn_enemies(mut commands: Commands) {
    let positions = [
        Vec2::new(-200.0, 150.0),
        Vec2::new(200.0, -150.0),
        Vec2::new(-150.0, -200.0),
    ];

    for pos in positions {
        commands.spawn((
            Enemy {
                direction: Vec2::new(0.6, 0.8).normalize(),
            },
            Sprite {
                color: Color::srgb(0.95, 0.2, 0.2),
                custom_size: Some(Vec2::splat(ENEMY_SIZE)),
                ..default()
            },
            Transform::from_xyz(pos.x, pos.y, 10.0),
            InGame,
        ));
    }
}

fn patrol_enemies(
    time: Res<Time>,
    bounds: Res<ArenaBounds>,
    mut query: Query<(&mut Transform, &mut Enemy)>,
) {
    let half = ENEMY_SIZE / 2.0;
    let min_x = bounds.min.x + half;
    let max_x = bounds.max.x - half;
    let min_y = bounds.min.y + half;
    let max_y = bounds.max.y - half;

    for (mut transform, mut enemy) in &mut query {
        let delta = enemy.direction * ENEMY_SPEED * time.delta_secs();
        let next = transform.translation.truncate() + delta;

        // Bounce off arena walls.
        if next.x < min_x || next.x > max_x {
            enemy.direction.x *= -1.0;
        }
        if next.y < min_y || next.y > max_y {
            enemy.direction.y *= -1.0;
        }

        transform.translation += enemy.direction.extend(0.0) * ENEMY_SPEED * time.delta_secs();
    }
}
