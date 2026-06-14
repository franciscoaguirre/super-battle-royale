# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

A 2D top-down game built with [Bevy](https://bevyengine.org/) `0.18.1` (Rust edition 2024). A fixed arena with a tiled floor, a movable player, patrolling enemies, and looping background music. It runs offline single-player **and** as authoritative client/server multiplayer (see [Multiplayer](#multiplayer-clientserver) and `README.md` for deployment).

## Commands

```bash
cargo run                    # offline single-player (windowed client)
cargo run -- <domain[:port]> # online client, connects to a server (port defaults to 5000)
cargo run --no-default-features --features server --bin server   # headless dedicated server

cargo build          # compile the client
cargo clippy                       # lint the client build
cargo clippy --features server     # lint client + server code
cargo fmt            # format
cargo test                   # run unit tests (offline build)
cargo test --features server # also runs the headless replication integration test
```

### Nix dev shell

The toolchain is pinned via `flake.nix` (nightly Rust `2026-05-09` with `rust-src` + `rust-analyzer`). Enter the environment before running cargo:

```bash
nix develop
```

On **Linux** the flake also provides Bevy's system dependencies (Wayland/X11, Vulkan, ALSA, udev) and the `shellHook` exports `LD_LIBRARY_PATH` — including `target/debug/deps`, which is required because of `dynamic_linking` (see gotchas). On macOS no extra libs are needed.

## Architecture

### Composition root

There are **two thin binaries** over one **library** crate (`lib.rs` → `src/game/`):
- `src/main.rs` — the windowed client (`DefaultPlugins`, window titled "Super Battle Royale"). Parses an optional server-domain CLI arg to pick offline vs. online, inserts the `NetRole` resource, adds `GamePlugin`, and adds `ClientNetPlugin` when online.
- `src/bin/server.rs` — the headless dedicated server (`MinimalPlugins` at 60 Hz + `StatesPlugin` + `LogPlugin`), inserts `NetRole::Server`, adds `GamePlugin` + `ServerNetPlugin`.

`GamePlugin` (`src/game/mod.rs`) is shared by both: it calls `init_state::<GameState>()`, adds the simulation subsystem plugins (`enemy`, `map`, `player`), and — **only in the client build** (`#[cfg(feature = "client")]`) — the render/audio plugins (`camera`, `footsteps`, `music`) plus `sync_netpos_to_transform`. The networking transport plugins are added by the binaries, not `GamePlugin`, since they depend on the chosen role.

### The plugin-per-subsystem pattern

This is the core convention — **follow it when adding features**. Each subsystem is one file under `src/game/` exposing an `XxxPlugin` (`camera`, `combat`, `crt`, `effects`, `enemy`, `footsteps`, `map`, `music`, `player`, `projectile`; networking lives in `src/game/net/`). To add a subsystem: create the module, define its `Plugin`, and add it to the appropriate `add_plugins((...))` group in `mod.rs` (gate render/audio-only plugins behind `#[cfg(feature = "client")]`). A plugin owns its components, constants, spawn system, and update systems.

### State + entity lifecycle

- `GameState` (`src/game/state.rs`) is the Bevy state machine. Only `Playing` exists today, but it's already wired so menu/loading/game-over screens can be added without restructuring.
- Spawn systems run on `OnEnter(GameState::Playing)`; per-frame systems are gated with `.run_if(in_state(GameState::Playing))`.
- **Every gameplay entity is tagged with the `InGame` marker component** (`mod.rs`). `cleanup_ingame` despawns all `InGame` entities on state exit, making transitions cheap and safe. Any new gameplay entity must carry `InGame`.

### World layout & shared constants

- The arena is centered at the origin. `ArenaBounds` (`src/game/map.rs`) is the authoritative outer rectangle; `player` and `enemy` import it to clamp/bounce movement. Wall tiles (`Tile::Wall`) are also solid: player movement is resolved with sliding and projectiles despawn on contact, both using `TileMap::circle_intersects_wall` on the authoritative side.
- Z-ordering by `Transform` z: floor `0.0`, walls `0.5`, player/enemies `1.0`, camera `1000.0`.
- The camera (`src/game/camera.rs`) is a fixed orthographic 2D camera using `ScalingMode::FixedVertical`, so the whole arena stays visible regardless of window size.

### Multiplayer (client/server)

Authoritative client/server via [`bevy_replicon`](https://docs.rs/bevy_replicon) `0.40` + [`bevy_replicon_renet`](https://docs.rs/bevy_replicon_renet) `0.16` (renet/netcode over UDP). Lives in `src/game/net/` (`mod.rs`, `protocol.rs`, `client.rs`, `server.rs`).

- **Three roles**, chosen at startup and stored in the `NetRole` resource: `Offline`, `OnlineClient`, `Server`. Run-conditions in `net/mod.rs` gate systems: `is_authoritative` (Offline ∨ Server — runs simulation), `is_offline`, `is_online_client`.
- **`NetPos(Vec2)` is the single source of truth for position** on every dynamic entity (players + enemies), in all modes. Authoritative sides write it from simulation; online clients receive it via replication. `sync_netpos_to_transform` (client only, `PostUpdate`) copies it into `Transform` — snapping offline, lerping online. **The server never uses `Transform`/rendering.**
- **Movement is split into intent + apply.** `PlayerIntent` (server-only component) holds the desired direction; `apply_player_intent` (authoritative) moves `NetPos`, sliding along wall tiles and clamping to `ArenaBounds`. Offline writes intent from local keys (`read_local_input`); online clients send a `PlayerInput` client-event each frame (`net/client.rs`), and the server applies it (`receive_input` in `net/server.rs`). The server clamps input magnitude to prevent speed hacks.
- **Replicated protocol** is registered once in `net::register_protocol`: components `NetPos`, `Player`, `PlayerColor`, `Enemy` (all derive `Serialize`/`Deserialize`); client-event `PlayerInput`. Replicon hashes this and rejects mismatched clients. **Anything you want replicated must be added here and derive serde.**
- **The map is NOT replicated.** It's deterministic and file-based, so client and server both `load_map()` and get identical `ArenaBounds`/spawn points; only dynamic entities replicate.
- **Players are the client entity.** On the server, connecting clients are entities; `on_client_authorized` (observer on `Add, AuthorizedClient`) attaches `Player`/`PlayerColor`/`NetPos`/`PlayerIntent`/`Replicated` to the client entity, so the renet backend auto-despawns the player (and propagates removal) on disconnect.
- **Client sprite attachment.** Server-spawned and replicated-in entities have no sprite; `attach_player_sprite`/`attach_enemy_sprite` (client only) add the `Sprite`/`Transform`/`InGame` to any matching entity that lacks one — covering both the offline local spawn and replicated remote entities through one path.

- **Shooting** (`src/game/projectile.rs`) reuses all of the above. Pressing **Space** fires a shot in the player's last-moved direction (`Facing`, a server/sim-only component updated from `PlayerIntent`). A shot flies straight at constant horizontal speed while its altitude (`Height`) sinks under gentle gravity, then despawns when it "crashes" into the ground or collides with a wall tile. `Projectile`, `Height`, and `ShotColor` (the firing player's color, so the shot/trail glow to match) are replicated (added in `register_protocol`); velocity is server/sim-only. Offline fires the local player directly; online sends a `ShootRequest` client-event and the server fires via the `receive_shoot` observer. Clients draw the descent with a child shadow (`render_projectiles`); projectiles are excluded from `sync_netpos_to_transform` because they carry altitude. No damage yet.

- **Combat** (`src/game/combat.rs`) is **PvP only** — the patrolling enemies ("bots") are deliberately ignored, so shots pass through them. A shot damages the first non-owner player it touches (within `HIT_RADIUS`, owner tracked via `ProjectileOwner` on the projectile). `Health` is server/sim-only (no HUD yet, so not replicated); at 0 HP a player gets the replicated `Dead` marker + a `RespawnTimer`, and after a short delay respawns at a spawn point with full health. `Dead` is replicated so clients hide dead players (`hide_dead_players`); dead players can't move or shoot (systems filter `Without<Dead>`). All of this is authoritative (server + offline); offline single-player simply has no other player to damage.

The integration test `tests/replication.rs` (run with `cargo test --features server`) connects a headless client+server over loopback and asserts replication, input, and the shoot→projectile flow end-to-end, plus a direct test of the damage→death combat loop.

### Visual effects (client-only)

A neon look layered on top of the sprites, all gated behind the `client` feature:
- **Bloom** (`camera.rs`): the camera has a `Bloom` component (auto-enables `Hdr`) with a `threshold` so only HDR-bright pixels glow. Projectiles/trails/sparks use `Color::linear_rgb` values > 1.0 to bloom; the scene art (≤ 1.0) doesn't. `Tonemapping::None` preserves the pixel-art colors.
- **CRT** (`crt.rs` + `crt.wgsl`): a custom fullscreen post-process node (scanlines + barrel curvature + vignette), modeled on Bevy's `effect_stack` node. Inserted in the `Core2d` graph between `Node2d::PostProcessing` and `Node2d::Tonemapping`, enabled by the `Crt` marker on the camera. Takes no uniforms.
- **Chromatic aberration** uses Bevy's built-in `ChromaticAberration` component (kept at intensity 0 at rest; pulsed on hits/deaths).
- **`effects.rs`** drives the rest from replicated signals — `Impact` markers (which carry a world position) and the `Dead` marker, the same hooks the audio uses: impact **sparks** + expanding **shockwave rings** (using procedurally-generated dot/ring textures), **screen shake** (trauma-based camera offset), and the chromatic-aberration **pulse**. Because they key off replicated state, they fire identically offline, on the host, and on every client.

### Assets

Loaded at spawn time via `asset_server.load("<path>")` with paths relative to `assets/`. Patterns in use:
- `PlayerColor` enum (`src/game/player.rs`) maps variants to `sphere_*.png` via `asset_path()`; the spawn color comes from the `SelectedColor` resource (default `Blue`) — the hook for future color selection.
- The floor uses `SpriteImageMode::Tiled` to repeat `floor-tiles.png` across the arena rather than stretch it (`src/game/arena.rs`).
- Background music spawns an `AudioPlayer` + `PlaybackSettings { mode: Loop, .. }` tagged `BackgroundMusic` (`src/game/music.rs`).

## Gotchas

- **Cargo features split client vs. server.** `bevy` is `default-features = false`; the `client` feature (default) enables the full 2D Bevy stack (`bevy/2d`, `dynamic_linking`, `mp3`, `serialize`), the `server` feature enables only headless bits (`bevy_state`, `bevy_log`, `std`, executor, `serialize`). Each `[[bin]]` has `required-features` so a build compiles only its binary. Render/audio-only modules and systems are `#[cfg(feature = "client")]`; `Song` stays shared (the parser needs it) but its audio playback is client-gated.
- **`dynamic_linking` is client-only.** It links Bevy as a shared lib for fast client iteration, so the client cannot run unless that dylib is on the library path (the flake's `shellHook` handles this on Linux). The **server** has no `dynamic_linking` and no render/audio features, so it builds statically with no Vulkan/ALSA/udev — ideal for deployment. Drop `dynamic_linking` for release client builds.
- **Audio formats**: Bevy decodes only OGG/Vorbis by default. The `mp3` feature is enabled (client only) for `shooter_loop.mp3`; add `flac`/`wav` features if those formats are needed. OGG loops more reliably (gapless) than MP3.
