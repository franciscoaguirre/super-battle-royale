//! A* pathfinding on the tile map.
//!
//! This module is simulation-only: the authoritative side runs pathfinding for
//! bots, and clients receive only the resulting [`NetPos`] updates.

use bevy::prelude::*;
use pathfinding::prelude::astar;

use super::map::{Tile, TileMap};

/// Searches for a path from `start` to `goal` that stays clear of wall tiles.
///
/// `radius` is the clearance required around the travelling circle (typically
/// `BOT_SIZE / 2`). The returned waypoints are world-space cell centres, from
/// the cell after `start` through to `goal`. If no path exists, `None` is
/// returned and the caller should fall back to straight-line movement.
pub fn find_path(map: &TileMap, start: Vec2, goal: Vec2, radius: f32) -> Option<Vec<Vec2>> {
    let (start_col, start_row) = map.world_to_cell(start);
    let (goal_col, goal_row) = map.world_to_cell(goal);
    let start_node = (start_col, start_row);
    let goal_node = (goal_col, goal_row);
    let (width, height) = map.dimensions();

    let result = astar(
        &start_node,
        |&(col, row)| successors(map, col, row, radius, width, height, goal_node),
        |&(col, row)| heuristic(col, row, goal_col, goal_row),
        |&node| node == goal_node,
    )?;

    Some(
        result
            .0
            .into_iter()
            .skip(1) // the first node is the start cell
            .map(|(col, row)| map.cell_center(col as usize, row as usize))
            .collect(),
    )
}

fn successors(
    map: &TileMap,
    col: i32,
    row: i32,
    radius: f32,
    width: usize,
    height: usize,
    goal: (i32, i32),
) -> Vec<((i32, i32), u32)> {
    const CARDINAL_COST: u32 = 10;
    const DIAGONAL_COST: u32 = 14; // approx sqrt(2) * 10

    #[rustfmt::skip]
    const DELTAS: [(i32, i32, u32); 8] = [
        ( 0,  1, CARDINAL_COST),
        ( 0, -1, CARDINAL_COST),
        ( 1,  0, CARDINAL_COST),
        (-1,  0, CARDINAL_COST),
        ( 1,  1, DIAGONAL_COST),
        ( 1, -1, DIAGONAL_COST),
        (-1,  1, DIAGONAL_COST),
        (-1, -1, DIAGONAL_COST),
    ];

    let mut result = Vec::new();
    let max_col = width as i32;
    let max_row = height as i32;

    for (dc, dr, cost) in DELTAS {
        let next_col = col + dc;
        let next_row = row + dr;

        if next_col < 0 || next_row < 0 || next_col >= max_col || next_row >= max_row {
            continue;
        }

        // Diagonal moves must not squeeze through an inside corner created by
        // two orthogonal walls.
        if dc != 0 && dr != 0 {
            let horizontal_clear = is_walkable_or_goal(map, col + dc, row, radius, goal);
            let vertical_clear = is_walkable_or_goal(map, col, row + dr, radius, goal);
            if !horizontal_clear || !vertical_clear {
                continue;
            }
        }

        if is_walkable_or_goal(map, next_col, next_row, radius, goal) {
            result.push(((next_col, next_row), cost));
        }
    }

    result
}

fn is_walkable_or_goal(map: &TileMap, col: i32, row: i32, radius: f32, goal: (i32, i32)) -> bool {
    (col, row) == goal || is_walkable(map, col, row, radius)
}

fn is_walkable(map: &TileMap, col: i32, row: i32, radius: f32) -> bool {
    let tile = map.tile_at(col, row);
    if matches!(tile, Tile::Wall | Tile::Void) {
        return false;
    }

    let center = map.cell_center(col as usize, row as usize);
    !map.circle_intersects_wall(center, radius)
}

fn heuristic(col: i32, row: i32, goal_col: i32, goal_row: i32) -> u32 {
    let dx = col.abs_diff(goal_col);
    let dy = row.abs_diff(goal_row);
    let diagonal = dx.min(dy);
    let straight = dx.max(dy) - diagonal;
    straight * 10 + diagonal * 14
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line_path_on_open_map() {
        let map = TileMap::parse("wwww\nwxxw\nwxxw\nwwww");
        let start = map.cell_center(1, 1);
        let goal = map.cell_center(2, 2);
        let path = find_path(&map, start, goal, 10.0).expect("path should exist");
        assert!(!path.is_empty());
        assert_eq!(path.last().copied(), Some(goal));
    }

    #[test]
    fn path_around_wall_requires_detour() {
        // A 5x5 map with a horizontal wall across the middle, leaving a gap on
        // the right side.
        let map = TileMap::parse(
            "wwwww\n\
             wxxww\n\
             wwxww\n\
             wxxww\n\
             wwwww",
        );
        let start = map.cell_center(1, 1);
        let goal = map.cell_center(1, 3);
        let path = find_path(&map, start, goal, 10.0).expect("path should exist");
        assert_eq!(path.last().copied(), Some(goal));
        // The detour should go through the gap at column 2.
        assert!(path.iter().any(|p| p.x > start.x + 32.0));
    }

    #[test]
    fn diagonal_corner_cut_is_rejected() {
        // Floor at (1,1) with walls at (2,1) and (1,2). The only valid path
        // from (1,1) to (2,2) must go the long way around, not cut the corner.
        let map = TileMap::parse(
            "wwww\n\
             wxww\n\
             wwxw\n\
             wwww",
        );
        let start = map.cell_center(1, 1);
        let goal = map.cell_center(2, 2);
        assert!(find_path(&map, start, goal, 10.0).is_none());
    }

    #[test]
    fn no_path_when_goal_enclosed() {
        let map = TileMap::parse(
            "wwwww\n\
             wwxxw\n\
             wwwww\n\
             wwxxw\n\
             wwwww",
        );
        // Start and goal are on opposite sides of a solid wall row.
        let start = map.cell_center(2, 1);
        let goal = map.cell_center(2, 3);
        assert!(find_path(&map, start, goal, 10.0).is_none());
    }
}
