use bevy_ggrs::GgrsConfig;
use bevy_matchbox::prelude::PeerId;
use serde::{Deserialize, Serialize};

pub const INPUT_UP: u8 = 1 << 0;
pub const INPUT_DOWN: u8 = 1 << 1;
pub const INPUT_LEFT: u8 = 1 << 2;
pub const INPUT_RIGHT: u8 = 1 << 3;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlayerInput {
    pub inp: u8,
}

pub type SbrConfig = GgrsConfig<PlayerInput, PeerId>;
