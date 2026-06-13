# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

A 2D top-down game built with [Bevy](https://bevyengine.org/) `0.18.1` (Rust edition 2024). Currently a playable skeleton: a fixed arena with a tiled floor, a movable player, patrolling enemies, and looping background music.

## Commands

```bash
cargo run            # build + launch the game
cargo build          # compile only
cargo clippy         # lint
cargo fmt            # format
cargo test           # run tests (none exist yet; harness is ready)
```

### Nix dev shell

The toolchain is pinned via `flake.nix` (nightly Rust `2026-05-09` with `rust-src` + `rust-analyzer`). Enter the environment before running cargo:

```bash
nix develop
```

On **Linux** the flake also provides Bevy's system dependencies (Wayland/X11, Vulkan, ALSA, udev) and the `shellHook` exports `LD_LIBRARY_PATH` — including `target/debug/deps`, which is required because of `dynamic_linking` (see gotchas). On macOS no extra libs are needed.

## Architecture

### Composition root

`main.rs` (binary) builds the `App` with `DefaultPlugins` (window titled "Super Battle Royale") and adds the single `GamePlugin`. All game logic lives in the **library** crate (`lib.rs` → `src/game/`), so the binary stays a thin entry point.

`GamePlugin` (`src/game/mod.rs`) is where everything is wired: it calls `init_state::<GameState>()`, adds every subsystem plugin, and registers `cleanup_ingame` on `OnExit(GameState::Playing)`.

### The plugin-per-subsystem pattern

This is the core convention — **follow it when adding features**. Each subsystem is one file under `src/game/` exposing an `XxxPlugin` (`arena`, `camera`, `enemy`, `music`, `player`). To add a subsystem: create the module, define its `Plugin`, and add it to the `add_plugins((...))` tuple in `mod.rs`. A plugin owns its components, constants, spawn system, and update systems.

### State + entity lifecycle

- `GameState` (`src/game/state.rs`) is the Bevy state machine. Only `Playing` exists today, but it's already wired so menu/loading/game-over screens can be added without restructuring.
- Spawn systems run on `OnEnter(GameState::Playing)`; per-frame systems are gated with `.run_if(in_state(GameState::Playing))`.
- **Every gameplay entity is tagged with the `InGame` marker component** (`mod.rs`). `cleanup_ingame` despawns all `InGame` entities on state exit, making transitions cheap and safe. Any new gameplay entity must carry `InGame`.

### World layout & shared constants

- The arena is centered at the origin. `ARENA_WIDTH`/`ARENA_HEIGHT` (`src/game/arena.rs`) are the authoritative bounds; `player` and `enemy` import them to clamp/bounce movement. `arena.rs` is the source of truth for world size.
- Z-ordering by `Transform` z: floor `0.0`, walls `0.5`, player/enemies `1.0`, camera `1000.0`.
- The camera (`src/game/camera.rs`) is a fixed orthographic 2D camera using `ScalingMode::FixedVertical`, so the whole arena stays visible regardless of window size.

### Assets

Loaded at spawn time via `asset_server.load("<path>")` with paths relative to `assets/`. Patterns in use:
- `PlayerColor` enum (`src/game/player.rs`) maps variants to `sphere_*.png` via `asset_path()`; the spawn color comes from the `SelectedColor` resource (default `Blue`) — the hook for future color selection.
- The floor uses `SpriteImageMode::Tiled` to repeat `floor-tiles.png` across the arena rather than stretch it (`src/game/arena.rs`).
- Background music spawns an `AudioPlayer` + `PlaybackSettings { mode: Loop, .. }` tagged `BackgroundMusic` (`src/game/music.rs`).

## Gotchas

- **`dynamic_linking`** is enabled in `Cargo.toml` for fast iterative builds. The binary links Bevy as a shared lib, so it cannot run unless that dylib is on the library path (the flake's `shellHook` handles this on Linux). Drop the feature for release/distribution builds.
- **Audio formats**: Bevy decodes only OGG/Vorbis by default. The `mp3` feature is enabled in `Cargo.toml` for `shooter_loop.mp3`; add `flac`/`wav` features if those formats are needed. OGG loops more reliably (gapless) than MP3.
