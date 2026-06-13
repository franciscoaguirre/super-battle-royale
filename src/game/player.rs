use bevy::prelude::*;

use super::InGame;
use super::arena::{ARENA_HEIGHT, ARENA_WIDTH};

pub const PLAYER_SIZE: f32 = 32.0;
const PLAYER_SPEED: f32 = 240.0;

#[derive(Component)]
pub struct Player;

/// The visual color of a player. More variants can be chosen by the player once
/// color selection is implemented.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlayerColor {
    Red,
    #[default]
    Blue,
    Green,
    Orange,
    Purple,
    Yellow,
}

impl PlayerColor {
    /// Path to the sprite for this color, relative to the `assets/` dir.
    pub fn asset_path(self) -> &'static str {
        match self {
            PlayerColor::Red => "sphere_red.png",
            PlayerColor::Blue => "sphere_blue.png",
            PlayerColor::Green => "sphere_green.png",
            PlayerColor::Orange => "sphere_orange.png",
            PlayerColor::Purple => "sphere_purple.png",
            PlayerColor::Yellow => "sphere_yellow.png",
        }
    }
}

/// The color the next player will spawn with. Defaults to [`PlayerColor::Blue`].
#[derive(Resource, Default)]
pub struct SelectedColor(pub PlayerColor);

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedColor>()
            .add_systems(OnEnter(super::state::GameState::Playing), spawn_player)
            .add_systems(
                Update,
                move_player.run_if(in_state(super::state::GameState::Playing)),
            );
    }
}

fn spawn_player(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    selected: Res<SelectedColor>,
) {
    let color = selected.0;
    commands.spawn((
        Player,
        color,
        Sprite {
            image: asset_server.load(color.asset_path()),
            custom_size: Some(Vec2::splat(PLAYER_SIZE)),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 1.0),
        InGame,
    ));
}

fn move_player(
    input: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut query: Query<&mut Transform, With<Player>>,
) {
    let mut direction = Vec2::ZERO;

    if input.pressed(KeyCode::KeyW) || input.pressed(KeyCode::ArrowUp) {
        direction.y += 1.0;
    }
    if input.pressed(KeyCode::KeyS) || input.pressed(KeyCode::ArrowDown) {
        direction.y -= 1.0;
    }
    if input.pressed(KeyCode::KeyA) || input.pressed(KeyCode::ArrowLeft) {
        direction.x -= 1.0;
    }
    if input.pressed(KeyCode::KeyD) || input.pressed(KeyCode::ArrowRight) {
        direction.x += 1.0;
    }

    if direction != Vec2::ZERO {
        direction = direction.normalize();
    }

    let half = PLAYER_SIZE / 2.0;
    let min_x = -ARENA_WIDTH / 2.0 + half;
    let max_x = ARENA_WIDTH / 2.0 - half;
    let min_y = -ARENA_HEIGHT / 2.0 + half;
    let max_y = ARENA_HEIGHT / 2.0 - half;

    for mut transform in &mut query {
        transform.translation += direction.extend(0.0) * PLAYER_SPEED * time.delta_secs();
        transform.translation.x = transform.translation.x.clamp(min_x, max_x);
        transform.translation.y = transform.translation.y.clamp(min_y, max_y);
    }
}
