# Super Battle Royale

A 2D top-down game built with [Bevy](https://bevyengine.org/) `0.18`. It runs in
three modes from a single codebase:

- **Offline single-player** — the original local game.
- **Online client** — connects to a dedicated server.
- **Dedicated server** — a headless, authoritative simulation you can deploy.

## Running locally

```bash
# Offline single-player (windowed)
cargo run --bin super-battle-royale

# Dedicated server (headless), listening on 0.0.0.0:5000
cargo run --no-default-features --features server --bin server

# Online client connecting to a server (windowed)
cargo run --bin super-battle-royale -- 127.0.0.1:5000
```

The client takes a single argument: the server's **domain** (or IP), optionally
with `:port` (the port defaults to `5000`). For example:

```bash
cargo run --bin super-battle-royale -- play.example.com          # connects to play.example.com:5000
cargo run --bin super-battle-royale -- play.example.com:7777     # custom port
```

With no argument the client runs offline single-player.

For a one-command multiplayer test, `scripts/dev.sh` builds everything, starts a
local server, and launches two clients connected to it (Ctrl-C tears it down):

```bash
scripts/dev.sh        # or: scripts/dev.sh <port>
```

## Controls

- **Move:** WASD or arrow keys.
- **Shoot:** Space — fires a shot in your last-moved direction. Shots fly straight
  and slowly sink under gravity, crashing into the ground after a short distance.

## Architecture

This is a server-authoritative design backed by
[`bevy_replicon`](https://docs.rs/bevy_replicon) +
[`bevy_replicon_renet`](https://docs.rs/bevy_replicon_renet) (renet/netcode over
UDP).

- The **server** owns the simulation: it moves players (from the input each
  client sends) and patrols enemies, then replicates their positions to all
  clients.
- **Clients** render the replicated world and send their input; they run no local
  simulation while online.
- The **map** is deterministic and loaded from `assets/maps/arena.txt` on both
  sides, so only dynamic entities (players, enemies) are replicated — never the
  map geometry.

The client/server split is done with Cargo features so the server compiles
**without** any rendering, audio, or windowing dependencies (no Vulkan / ALSA /
udev needed on the host):

| Binary | Build | Plugins |
|--------|-------|---------|
| `super-battle-royale` (client) | `--features client` (default) | `DefaultPlugins` |
| `server` | `--no-default-features --features server` | `MinimalPlugins` (headless 60 Hz) |

See `src/game/net/` for the networking module and `CLAUDE.md` for the broader
codebase guide.

## Deploying the server

The server is a headless binary intended to run on a small Linux host.

### 1. Build a release binary

On the target Linux host (or a matching build box):

```bash
cargo build --release --no-default-features --features server --bin server
```

The headless feature set links no GPU/audio/window crates, so a vanilla Linux box
with the Rust toolchain and a C toolchain is enough — you do **not** need Vulkan,
ALSA, or udev development libraries. The resulting binary is at
`target/release/server`. It is statically linked against Bevy (the
`dynamic_linking` feature is client-only), so it is self-contained.

### 2. Ship the map (optional but recommended)

Copy `assets/maps/arena.txt` next to the binary's working directory so the server
loads the exact same map as clients. If the file is missing the server falls back
to the built-in default map, which must match the clients' map. Keep
`assets/maps/arena.txt` and the `DEFAULT_MAP` constant in `src/game/map.rs` in
sync.

### 3. Run it and open the port

```bash
./server 0.0.0.0:5000
```

Open **UDP** port `5000` (the game uses UDP, not TCP) in the host firewall and any
cloud security group / NAT:

```bash
# Example: ufw
sudo ufw allow 5000/udp
```

### 4. Point your domain at it

Create a DNS **A record** (e.g. `play.yourdomain.com`) pointing at the host's
public IP. Players then connect with:

```bash
super-battle-royale play.yourdomain.com
```

### Optional: run under systemd

```ini
# /etc/systemd/system/sbr-server.service
[Unit]
Description=Super Battle Royale dedicated server
After=network-online.target
Wants=network-online.target

[Service]
# Working directory should contain ./server and ./assets/maps/arena.txt
WorkingDirectory=/opt/sbr
ExecStart=/opt/sbr/server 0.0.0.0:5000
Restart=on-failure
User=sbr

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now sbr-server
journalctl -u sbr-server -f   # follow logs
```

### Security note

The server uses renet's **unsecure** netcode authentication (a shared protocol
id, no per-client keys). This is fine for a hobby/open server but offers no
authentication or encryption. For a hardened deployment, switch to renet's secure
authentication (connect tokens with a private key) — see
`src/game/net/server.rs` (`ServerAuthentication`).
