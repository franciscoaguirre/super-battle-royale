# Option: Generic `NetworkBackend` trait

## Core idea

Instead of asking "which `NetRole` am I?" at runtime, make `GamePlugin` generic over a `NetworkBackend` type. The backend is chosen once at startup and compiled into the app, so gameplay code never branches on `Offline` / `OnlineClient` / `Server`.

```rust
// main.rs
app.add_plugins(GamePlugin::<OfflineBackend>::new());

// server.rs
app.add_plugins(GamePlugin::<ServerBackend>::new());
```

## Trait sketch

```rust
/// Everything the gameplay code needs from the network layer.
pub trait NetworkBackend: Send + Sync + 'static {
    /// Name for logging/debugging.
    const NAME: &'static str;

    /// Is this instance authoritative (offline + server)?
    const IS_AUTHORITATIVE: bool;

    /// Is this instance rendering (offline + client)?
    const IS_CLIENT: bool;

    /// Register a replicated component.
    fn register_replicated<C>(&self, app: &mut App)
    where
        C: Component + Serialize + DeserializeOwned + Clone + Send + Sync + 'static;

    /// Register a client→server event.
    fn register_client_event<E>(&self, app: &mut App)
    where
        E: Event + Serialize + DeserializeOwned + Send + Sync + 'static;

    /// Register a server→client event.
    fn register_server_event<E>(&self, app: &mut App)
    where
        E: Event + Serialize + DeserializeOwned + Send + Sync + 'static;

    /// Spawn a gameplay entity. Offline/server apply `InGame`; server also marks `Replicated`.
    fn spawn_actor<B: Bundle>(&self, commands: &mut Commands, bundle: B) -> EntityCommands<'_>;

    /// Route local movement input. Offline writes `PlayerIntent`; client sends `PlayerInput`.
    fn apply_movement_input(&self, commands: &mut Commands, dir: Vec2, seq: Option<u32>);

    /// Route a local shoot request. Offline fires directly; client sends `ShootRequest`.
    fn apply_shoot_input(&self, commands: &mut Commands);

    /// Route a local shield request. Offline sets `ShieldState`; client sends `ShieldRequest`.
    fn apply_shield_input(&self, commands: &mut Commands, active: bool);
}
```

## Concrete backends

```rust
pub struct OfflineBackend;
pub struct ClientBackend;
pub struct ServerBackend;

impl NetworkBackend for OfflineBackend {
    const NAME: &'static str = "offline";
    const IS_AUTHORITATIVE: bool = true;
    const IS_CLIENT: bool = true;

    fn register_replicated<C>(&self, app: &mut App)
    where
        C: Component + Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
    {
        // Offline: no replication, but we still need to register with Replicon
        // so the component exists in the type registry if we ever want LAN.
        // Or just no-op if truly local-only.
    }

    fn spawn_actor<B: Bundle>(&self, commands: &mut Commands, bundle: B) -> EntityCommands<'_> {
        commands.spawn((InGame, bundle))
    }

    fn apply_movement_input(&self, commands: &mut Commands, dir: Vec2, _seq: Option<u32>) {
        commands.insert_resource(NextPlayerIntent(dir));
    }

    // ...
}
```

The `ClientBackend` and `ServerBackend` implementations would call `bevy_replicon` / `bevy_replicon_renet` methods.

## How plugins change

### `GamePlugin<B: NetworkBackend>`

```rust
pub struct GamePlugin<B: NetworkBackend> {
    _backend: PhantomData<B>,
}

impl<B: NetworkBackend> Plugin for GamePlugin<B> {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            CombatPlugin::<B>::new(),
            BotPlugin::<B>::new(),
            MapPlugin,
            MatchFlowPlugin::<B>::new(),
            PickupPlugin,
            PlayerPlugin::<B>::new(),
            ProjectilePlugin::<B>::new(),
            ShieldPlugin,
        ));

        if B::IS_CLIENT {
            app.add_plugins((CameraPlugin, CrtPlugin, EffectsPlugin, ...));
        }
    }
}
```

### Generic systems

Instead of run conditions, systems are generic:

```rust
fn apply_player_intent<B: NetworkBackend>(
    backend: Res<B>,
    // ...
) {
    // no runtime role check; behavior is in backend methods
}
```

Systems that only exist on one backend are still added conditionally, but the condition is now a compile-time constant:

```rust
if B::IS_AUTHORITATIVE {
    app.add_systems(FixedUpdate, apply_player_intent::<B>);
}
```

### `NetworkBackend` resource

The backend instance is inserted as a resource so systems can call it:

```rust
app.insert_resource(OfflineBackend);
```

Systems take `backend: Res<B>`.

## How spawn becomes generic

Replace `SpawnContext` with the backend trait:

```rust
fn spawn_player<B: NetworkBackend>(
    mut commands: Commands,
    backend: Res<B>,
    selected: Res<SelectedColor>,
    map: Res<CurrentMap>,
) {
    let spawn = map.0.spawn_points().first().copied().unwrap_or(Vec2::ZERO);
    let entity = backend
        .spawn_actor(
            &mut commands,
            (Player, selected.0, NetPos(spawn), PlayerIntent::default()),
        )
        .id();
    insert_shield(&mut commands, entity);
}
```

The `Replicated` derive and `NetworkRegistry` can stay; the backend just calls `register_replicated` for each type.

## How input becomes generic

The `InputBackend` refactor becomes unnecessary. We have a single sampler and a generic router:

```rust
fn sample_local_input<B: NetworkBackend>(
    input: Res<ButtonInput<KeyCode>>,
    backend: Res<B>,
    mut commands: Commands,
) {
    if !B::IS_CLIENT {
        return; // compile-time branch; server has no keyboard
    }

    let dir = input_direction(&input);
    let shoot = input.just_pressed(KeyCode::Space);
    let shield_pressed = input.pressed(KeyCode::ShiftLeft) || input.pressed(KeyCode::ShiftRight);

    backend.apply_movement_input(&mut commands, dir, None);
    if shoot {
        backend.apply_shoot_input(&mut commands);
    }
    // shield transition tracking omitted for brevity
}
```

## Pros

- **Compile-time guarantees**: you cannot forget to handle a backend for a given operation; the trait forces you to implement it.
- **No runtime role checks**: `is_authoritative`, `is_offline`, `is_online_client` disappear from gameplay systems.
- **Single plugin graph**: `GamePlugin<B>` is the same plugin tree for all roles, compiled differently.
- **Easier to add a new backend**: e.g. a LAN listener or a replay recorder just needs a new `impl NetworkBackend`.
- **Type-safe spawn**: `spawn_actor` returns `EntityCommands`, and the backend decides replication/`InGame` policy.

## Cons

- **Generic plugins in Bevy are noisy**: every system, plugin, and query type needs a `<B: NetworkBackend>` parameter. This adds a lot of syntactic overhead.
- **Bigger compile units**: `GamePlugin::<OfflineBackend>` and `GamePlugin::<ServerBackend>` are two different monomorphizations, potentially increasing compile time.
- **Harder to unit-test individual plugins**: tests must pick a concrete backend or implement a test double.
- **Not all differences are backend-shaped**: some code is client-only for rendering, some is server-only for headless loop. Constants like `IS_CLIENT` help, but you still end up with `if B::IS_CLIENT { ... }` in plugin build.
- **Replicon integration is awkward**: Replicon wants to register types directly on `App`. The backend trait has to wrap those calls, which is mostly boilerplate.

## Implementation sketch

1. Define `NetworkBackend` in `src/game/net/backend.rs`.
2. Implement `OfflineBackend`, `ClientBackend`, `ServerBackend` in `src/game/net/backends/`.
3. Make `GamePlugin` generic over `B: NetworkBackend`.
4. Convert sub-plugins (`PlayerPlugin`, `ProjectilePlugin`, etc.) to generic plugins.
5. Replace `SpawnContext`/`SpawnCommandsExt` with `backend.spawn_actor()`.
6. Remove `NetRole`, `is_authoritative`, `is_offline`, `is_online_client`, `InputBackend`.
7. Main/server binaries instantiate the right backend and plugin.

## Trade-off summary

| Concern | Current run-condition approach | Generic backend approach |
|--------|-------------------------------|--------------------------|
| Runtime overhead | Many `Res<NetRole>` checks | None (compile-time) |
| Compile-time safety | Easy to forget a branch | Trait forces handling |
| Code readability | Run conditions scatter role logic | Generics scatter `<B>` everywhere |
| Testability | Easy to build single plugins | Need a concrete backend or mock |
| Adding a new backend | Add run conditions / match arms | Add one `impl NetworkBackend` |
| Bevy ergonomics | Good | Mediocre (generic systems/plugins) |

## Verification plan

After implementation:

```bash
cargo test --features server
cargo test
cargo clippy --features server
```

A good smoke test is also running the offline client and the dedicated server against each other to ensure the generic dispatch does not break input/shoot/shield behavior.
