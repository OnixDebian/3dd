# Architecture Research

**Domain:** Rust TUI app combining an async data stream (Docker daemon) with a software 3D render loop (ratatui + braille rasterizer)
**Researched:** 2026-05-26
**Confidence:** HIGH (loop architecture, async/sync integration, Canvas pixel API, math lib); MEDIUM (3D rasterization-to-braille mapping, layout algorithm)

## Standard Architecture

### The Core Problem and Its Solution

Three concurrent concerns must never block each other:

- **(a)** async Docker streams (stats ~1-2 Hz, events sporadic) — lives in Tokio tasks
- **(b)** a sync software render loop at 30-60 fps (smooth camera motion)
- **(c)** responsive input (camera, selection, mode/theme toggles)

The proven ratatui-async pattern solves this with **two channels into one single-consumer main loop**:

1. A `tokio::select!` event task multiplexes **crossterm input + a tick interval + a render interval** into one `mpsc` channel (`Event::Key`, `Event::Tick`, `Event::Render`, `Event::Resize`).
2. A **separate Docker task** owns the bollard connection, consumes the stats/events streams, translates them to domain messages, and pushes them through a **second `mpsc` channel** (`DockerMsg`).
3. The **main loop** (`recv().await` on both, or `select!` over both) is the *only* consumer. It mutates `App` state and calls the synchronous `terminal.draw(...)` only on `Event::Render`.

The terminal `draw` closure is sync and reads `&App` only — it never `.await`s, never touches the network, and never blocks. This is the canonical pattern in the ratatui async-template and "Full Async Events" tutorial. (HIGH — ratatui official docs.)

```
┌────────────────────────────────────────────────────────────────────┐
│  ASYNC PRODUCERS (Tokio tasks, never touch the terminal)            │
│                                                                      │
│  ┌──────────────────┐                ┌───────────────────────────┐  │
│  │ Event task        │                │ Docker task (bollard)     │  │
│  │ tokio::select! {  │                │  - events() stream        │  │
│  │   crossterm input │                │  - stats(id) per container│  │
│  │   tick_interval   │                │  - periodic list_* polls  │  │
│  │   render_interval │                │  -> map to domain         │  │
│  │ }                 │                │                           │  │
│  └────────┬──────────┘                └───────────┬───────────────┘  │
│           │ Event (Key/Tick/Render/Resize)        │ DockerMsg         │
│           │ mpsc                                   │ mpsc             │
└───────────┼──────────────────────────────────────┼───────────────────┘
            │                                        │
            ▼                                        ▼
┌────────────────────────────────────────────────────────────────────┐
│  MAIN LOOP (single consumer, owns App state)                        │
│                                                                      │
│   loop {                                                             │
│     select! { ev = events.recv(), msg = docker.recv() }             │
│       Event::Key   -> InputController -> mutate Camera/Mode/Sel     │
│       DockerMsg     -> WorldModel.apply() (set animation targets)   │
│       Event::Tick   -> WorldModel.step(dt) + Camera.step(dt)        │
│                        (advance interpolation/animation)            │
│       Event::Render -> terminal.draw(|f| view(f, &app))  ← SYNC     │
│   }                                                                  │
└──────────────────────────────────────┬───────────────────────────────┘
                                        │ &App (read-only)
                                        ▼
┌────────────────────────────────────────────────────────────────────┐
│  RENDER (synchronous, pure: state -> pixels)                        │
│                                                                      │
│   Camera (view+proj mat4) → 3D Pipeline → braille framebuffer       │
│                                   │                                  │
│   ratatui layout: Canvas(scene) + DetailPanel + StatusBar           │
└────────────────────────────────────────────────────────────────────┘
```

### Loop architecture: TEA-flavored, not strict Elm

Use a **hybrid**: the classic ratatui "App state + event loop + draw" structure, lightly TEA-flavored.

- Keep an **`Action`/message enum** as the bridge (input → `Action`, Docker → `DockerMsg`) so input mapping is decoupled from state mutation. This is the async-template's "command pattern" and it pays off for mode/theme toggles and testability.
- **Do NOT adopt pure Elm** (immutable `Model`, `update -> new Model`). The world model holds per-frame mutable animation/interpolation state and a depth buffer; rebuilding it immutably each frame is wasteful at 60 fps. Mutate in place.

**Verdict:** `App { world, camera, ui_state, theme }` mutated by an `update(action)` dispatcher, drawn by a pure `view(frame, &app)`. (HIGH — matches every production async ratatui template.)

### State sharing: channels, not `Arc<Mutex>`

Prefer **message-passing over shared state**. The Docker task should not share `Arc<Mutex<World>>` with the renderer:

- `Arc<Mutex>` risks the render thread blocking on a lock held during a stats burst, causing frame hitches — exactly the "stream blocks rendering" failure this design must avoid.
- With channels, the producer is fire-and-forget; the single-consumer main loop is the sole writer of `World`, so no locking at all. (MEDIUM-HIGH — standard Tokio actor/channel guidance; the ratatui templates use channels throughout.)

Use `tokio::sync::mpsc::unbounded_channel` for both Event and Docker channels (unbounded is fine here — message rates are low and bounded by Docker, and it avoids `.await` on `send` in the producer).

### Component Responsibilities

| Component | Responsibility | Implementation |
|-----------|----------------|----------------|
| **Docker layer** (`docker/`) | Own bollard `Docker`; poll `list_containers/networks/volumes/images`; subscribe `events()`; spawn one `stats(id)` stream per running container; map raw API types → domain `DockerSnapshot`/`StatSample`; push `DockerMsg`. Never touches UI. | bollard async streams + Tokio tasks + `mpsc::Sender` |
| **World model** (`world/`) | Domain truth: `Entity` list (containers, racks, networks, volumes, images, ports) with 3D `position`, current+target visual props (size, color). Owns layout assignment and animation/interpolation state. `apply(DockerMsg)` sets targets; `step(dt)` advances toward them. | Flat `Vec<Entity>` + index maps; `glam::Vec3` positions |
| **Camera/Input** (`camera/`, `input/`) | `InputController` maps keys → `Action`. `Camera` holds orbit state (yaw/pitch/radius/target), autopilot vs manual, produces `view: Mat4` and `proj: Mat4`. `step(dt)` advances autopilot orbit + smooths manual motion. | `glam::Mat4` (`look_at_rh`, `perspective_rh`) |
| **3D pipeline** (`render3d/`) | Pure function `(world, camera, viewport) -> Framebuffer`. Transform → project (clip → NDC → screen) → rasterize entities into a color+depth framebuffer at braille sub-pixel resolution. | Custom code over `glam`; per-pixel depth buffer |
| **UI/render** (`ui/`) | ratatui layout: a `Canvas` widget hosting a custom `Shape` that blits the framebuffer via `Painter::paint(x,y,color)`; plus `DetailPanel` and `StatusBar` widgets. Pure `view(frame, &app)`. | ratatui `Canvas`, `Block`, `Paragraph` |
| **Theme/config** (`theme/`, `config/`) | Load palette config (cyberpunk/green/notion); map entity status/heat → `Color`. Runtime-switchable (just swaps the active palette; next frame reflects it). | serde + config file |
| **App / main loop** (`app.rs`, `main.rs`) | Wire channels, own `App`, run the event loop, dispatch actions, call draw on Render. | `tokio::select!` over two `mpsc::Receiver`s |

## Recommended Project Structure

```
src/
├── main.rs              # entry: build Docker conn, spawn tasks, run App
├── app.rs               # App state struct + event loop + update(action) dispatcher
├── action.rs            # Action enum (input intents) + DockerMsg enum (data updates)
├── tui.rs               # Terminal setup/teardown + tokio::select! event task -> Event mpsc
│
├── docker/
│   ├── mod.rs           # spawn(): owns bollard Docker, runs stream tasks
│   ├── poll.rs          # periodic list_containers/networks/volumes/images
│   ├── streams.rs       # events() + per-container stats() subscriptions
│   └── domain.rs        # DockerSnapshot, StatSample, ContainerStatus (decoupled from bollard types)
│
├── world/
│   ├── mod.rs           # World { entities, indices }; apply(DockerMsg); step(dt)
│   ├── entity.rs        # Entity { kind, position, size_cur/target, color_cur/target }
│   ├── layout.rs        # rack/grid placement algorithm -> assigns Vec3 positions
│   └── anim.rs          # interpolation helpers (lerp size/color toward targets, "breathe")
│
├── camera/
│   ├── mod.rs           # Camera { yaw,pitch,radius,target,mode }; view()/proj(); step(dt)
│   └── autopilot.rs     # hands-free orbit path
│
├── render3d/
│   ├── mod.rs           # render(world, camera, viewport) -> Framebuffer
│   ├── framebuffer.rs   # Framebuffer { color: Vec<Color>, depth: Vec<f32>, w, h }
│   ├── project.rs       # mvp transform, clip, perspective divide, NDC->screen
│   └── raster.rs        # rasterize points/lines/quads with depth test
│
├── ui/
│   ├── mod.rs           # view(frame, &app): layout split
│   ├── scene.rs         # SceneShape: impl Shape, blits Framebuffer via Painter::paint
│   ├── detail_panel.rs  # selected-entity detail widget
│   └── status_bar.rs    # mode / fps / theme / container count
│
├── input/
│   └── mod.rs           # InputController: KeyEvent -> Action; mode switching
│
├── theme/
│   └── mod.rs           # Palette; status/heat -> Color
│
└── config/
    └── mod.rs           # serde config load (theme choice, rates)
```

### Structure Rationale

- **`docker/domain.rs` isolates bollard types.** The world model speaks `ContainerStatus`, not bollard's `ContainerStateStatusEnum`. This keeps the renderer testable without a daemon and survives bollard API churn. (Anti-pattern to avoid: leaking bollard types into `world/` and `ui/`.)
- **`render3d/` is a pure module** — `render(world, camera, viewport) -> Framebuffer` with no I/O, no globals. Unit-testable and the heart of the aesthetic.
- **`world/` owns layout AND animation** because both are "domain truth over time," and both need to be stepped at tick rate independent of render rate.
- **`ui/scene.rs` is the only bridge** from framebuffer to ratatui — a single thin adapter.

## Architectural Patterns

### Pattern 1: Two-channel single-consumer loop (async producer → sync render)

**What:** All async work happens in spawned tasks that only *send* messages. One main loop owns all mutable state and is the sole consumer/renderer.
**When to use:** Any TUI mixing live data with a render loop — this exact case.
**Trade-offs:** + No locks, no data races, trivial reasoning, render never blocks on I/O. − All state lives in one place (fine for this app's scale).

```rust
loop {
    tokio::select! {
        Some(ev)  = event_rx.recv()  => match ev {
            Event::Key(k)   => app.update(input.map(k)),
            Event::Tick     => { app.world.step(dt); app.camera.step(dt); }
            Event::Render   => { terminal.draw(|f| ui::view(f, &app))?; }
            Event::Resize(..) => app.on_resize(..),
            _ => {}
        },
        Some(msg) = docker_rx.recv() => app.world.apply(msg), // set anim targets
    }
    if app.should_quit { break; }
}
```
(HIGH — ratatui "Full Async Events" + async-template.)

### Pattern 2: Fixed-timestep simulation + interpolated render (decouple stats rate from fps)

**What:** Stats arrive slowly (~1-2 Hz) and sporadically; render runs at 30-60 fps. Treat incoming stats as **animation targets**, advance current values toward targets every `Tick`, and render the *current* (interpolated) values. The classic Gaffer "Fix Your Timestep!" decoupling.
**When to use:** Whenever update cadence ≠ display cadence and you want smooth visuals — the "breathing boxes" requirement.
**Trade-offs:** + Buttery animation from coarse data; camera stays smooth regardless of Docker latency. − Must store both `current` and `target` per animated property.

```rust
// world/anim.rs — called on Event::Tick (or per-frame with dt)
entity.size_cur  = lerp(entity.size_cur,  entity.size_target,  1.0 - (-k*dt).exp());
entity.color_cur = lerp_color(entity.color_cur, entity.color_target, ...);
// DockerMsg only ever sets *_target; rendering reads *_cur.
```
**Where animation state lives:** in `world/` on each `Entity` (`*_cur` + `*_target`), advanced by `World::step(dt)`. Camera smoothing state lives in `camera/`. Neither lives in `render3d/` (which stays a pure snapshot renderer). (HIGH — Gaffer On Games; standard game-loop practice.)

### Pattern 3: Framebuffer → braille via a custom `Shape`

**What:** The 3D pipeline writes into an offscreen `Framebuffer { color, depth }` sized to the Canvas's braille resolution (2×4 sub-pixels per cell). A `SceneShape: Shape` then iterates the framebuffer and calls `Painter::paint(gx, gy, color)` for each lit sub-pixel. ratatui's Canvas already maps sub-character braille dots; `Painter::paint(x: usize, y: usize, color)` sets one dot, `get_point(fx, fy)` maps canvas coords → grid cell.
**When to use:** Rendering a software-rasterized scene inside ratatui (the rust-sloth approach, adapted to braille). (MEDIUM — Canvas/Painter API verified HIGH; the framebuffer-blit integration is the standard idiom but project-specific.)
**Trade-offs:** + Reuses ratatui's braille packing and diffing for free; clean separation. − Two coordinate systems (canvas math-coords bottom-left vs grid top-left) — centralize the mapping in `scene.rs`.

```rust
impl Shape for SceneShape<'_> {
    fn draw(&self, painter: &mut Painter) {
        for y in 0..self.fb.h {
            for x in 0..self.fb.w {
                if let Some(c) = self.fb.color_at(x, y) {
                    painter.paint(x, y, c); // braille sub-pixel
                }
            }
        }
    }
}
```

### Pattern 4: Flat entity list, not a scene graph or ECS

**What:** Store entities in a `Vec<Entity>` with index maps (by container id, by network). No parent/child transform hierarchy, no ECS crate.
**When to use:** Scenes with tens-to-low-hundreds of objects and *flat* spatial relationships — racks at world positions, blades at rack-relative offsets resolved once at layout time.
**Trade-offs:** + Simplest possible; cache-friendly iteration for projection; trivial to reason about. − No automatic transform inheritance (not needed — bake rack offset into each blade's world position at layout). An ECS (bevy_ecs/hecs) is **overkill** here and would obscure the render pipeline. (MEDIUM — judgment call; flat lists are standard for sub-1k-object software renderers like rust-sloth.)

## Data Flow

### Docker data → pixels (the full pipeline)

```
Docker daemon
  │  events() stream        stats(id) stream         list_* poll (every ~2s)
  ▼                          ▼                          ▼
docker/streams.rs + poll.rs  ── map → domain ──►  DockerMsg
                                                     │ mpsc
                                                     ▼
World::apply(msg)   →  add/remove Entity, set size_target/color_target, layout new ones
                                                     │
        Event::Tick ──► World::step(dt): size_cur→target, color_cur→target, breathe
                        Camera::step(dt): advance autopilot orbit / smooth manual
                                                     │
        Event::Render ──────────────────────────────┘
                                                     ▼
render3d::render(&world, &camera, viewport):
   for entity in world.entities:
       model_mat (from entity.position + size_cur)
       mvp = proj * view * model
       project verts → clip → /w → NDC → screen(braille px)
       rasterize quad/point with depth test → Framebuffer{color_cur, depth}
                                                     ▼
ui::view: Canvas.paint(|ctx| ctx.draw(&SceneShape{fb}))  +  DetailPanel(&selected) + StatusBar
                                                     ▼
ratatui diffs Buffer → terminal
```

### State management

```
Single source of truth: App { world, camera, ui_state, theme }
  - Only the main loop mutates it (no Arc<Mutex>).
  - DockerMsg  → mutate world (targets).
  - Action     → mutate camera / ui_state / theme.
  - Tick       → step animation.
  - Render     → read-only view(&app).
```

### Key data flows

1. **Stats breathe:** `stats()` sample → `size_target` → `step(dt)` lerps `size_cur` → box scale → smooth pulsing even though data is 1-2 Hz.
2. **Status color:** `events()` (die/start/pause) → `color_target` → cross-fade.
3. **Selection/detail:** `Tab` → `Action::CycleSelection` → `ui_state.selected` → `DetailPanel` reads that entity's domain data; 3D pipeline highlights it.
4. **Theme switch:** `Action::CycleTheme` → swap active `Palette`; next frame's color mapping uses it (no rebuild).
5. **Mode switch:** any movement key while in autopilot → `Action::EnterManual` → `camera.mode = Manual`.

## Frame Timing

| Concern | Rate | Mechanism |
|---------|------|-----------|
| Render | 30-60 fps | `render_interval` in the select task → `Event::Render` → `draw` |
| Animation/camera step | tied to fps or fixed tick | `Event::Tick` (or compute `dt` per render) → `step(dt)` |
| Docker stats | ~1-2 Hz (daemon-driven) | bollard `stats()` stream, async, independent |
| Docker list poll | ~0.5 Hz | `tokio::time::interval` in poll task |
| Input | immediate | crossterm `EventStream`, dispatched the instant it arrives |

**Recommendation:** **render-on-interval with interpolation**, not pure render-on-event. A pure event-driven redraw can't animate autopilot orbit or breathing. Drive a steady `render_interval` (start 30 fps; it's a terminal — 30 is smooth and CPU-cheap, honoring the "must not peg CPU" constraint), advance animation by real `dt`, and let Docker run at whatever rate the daemon provides. The two are fully decoupled because they enter the loop through different channels. (HIGH — ratatui render-interval pattern + Gaffer interpolation.)

## Suggested Build Order (→ phase ordering)

Dependency-ordered for a lean **4-5 phase** roadmap. Each phase ends in something runnable/visible.

1. **Phase 1 — Skeleton loop + render plumbing (foundation).**
   `main.rs`, `tui.rs` (select! event task), `app.rs`, `action.rs`, ratatui terminal setup/teardown, `StatusBar`, quit handling. Draw a placeholder in a `Canvas`. *Proves the async event loop + render cadence before any 3D or Docker.* Everything depends on this.

2. **Phase 2 — 3D pipeline on fake data.**
   `render3d/` (framebuffer, project, raster), `camera/` (view+proj via glam, manual orbit + autopilot), `ui/scene.rs` (framebuffer→braille `Shape`), `world/` with **hardcoded** entities + `layout.rs`. *Delivers the core aesthetic: a rotating braille scene you can orbit.* Depends on P1. This is the riskiest/most novel part — do it early on synthetic data so it isn't blocked on Docker.

3. **Phase 3 — Docker data layer → live world.**
   `docker/` (poll + events + stats streams, `domain.rs`), `DockerMsg` channel, `World::apply`. Replace fake entities with real containers; map status→color, CPU/RAM→size. *Delivers: the real scene.* Depends on P2 (needs the world model + render to show anything).

4. **Phase 4 — Animation + interaction polish.**
   `world/anim.rs` (interpolation/breathe targets), camera smoothing, `input/` selection (`Tab`/`Enter`), `ui/detail_panel.rs`, networks/volumes/images/ports visualization. *Delivers: living, explorable scene.* Depends on P3 (animates real data; detail panel needs domain data).

5. **Phase 5 — Theming + config.**
   `theme/` palettes, `config/` loading, runtime theme switching, autopilot tuning. *Delivers: the configurable final look.* Depends on P4 (palettes apply to the full entity set).

**Critical path:** P1 (loop) → P2 (render) → P3 (data) → P4 (animate/interact) → P5 (theme). P2 before P3 is the key call: validate the hard, novel 3D-in-braille work on synthetic data so a Docker hiccup never blocks the visual core. If the roadmap must compress to 4 phases, merge P5 into P4.

## Anti-Patterns

### Anti-Pattern 1: Sharing `Arc<Mutex<World>>` between Docker task and renderer
**What people do:** Let the stats task lock and write the world the renderer reads.
**Why it's wrong:** A lock held during a stats burst stalls `draw`, producing frame hitches — the exact "stream blocks rendering" failure to avoid.
**Do this instead:** Channels. Docker task sends `DockerMsg`; the single main-loop consumer is the only writer. Zero locks.

### Anti-Pattern 2: `.await` inside the draw path
**What people do:** Fetch/inspect Docker data lazily during `view()`.
**Why it's wrong:** `terminal.draw` is sync and must be instant; any await there freezes the UI.
**Do this instead:** All data is pre-fetched into `World` by tasks. `view(&app)` is pure and synchronous.

### Anti-Pattern 3: Rendering directly from raw stats (no interpolation layer)
**What people do:** Map the latest stat sample straight to box size each frame.
**Why it's wrong:** 1-2 Hz data renders as visible jumps/stutter, killing the "breathing" aesthetic.
**Do this instead:** Stats set `*_target`; `step(dt)` lerps `*_cur`; render reads `*_cur`.

### Anti-Pattern 4: Leaking bollard types into world/render/ui
**What people do:** Pass `ContainerSummary`/`ContainerStatsResponse` deep into rendering.
**Why it's wrong:** Couples the visual core to a volatile external API; can't test without a daemon.
**Do this instead:** Translate to a small domain model in `docker/domain.rs`; everything downstream speaks domain types.

### Anti-Pattern 5: Reaching for an ECS or scene graph
**What people do:** Pull in bevy_ecs/hecs or build a transform hierarchy "for scalability."
**Why it's wrong:** Tens-to-hundreds of flat entities don't need it; it obscures the projection pipeline and adds deps.
**Do this instead:** `Vec<Entity>` with baked world positions from `layout.rs`.

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| Docker daemon | bollard `Docker::connect_with_local_defaults()`; `events()` + `stats(id, opts{stream:true})` streams + periodic `list_*` polls, all in Tokio tasks | Read-only. One stats stream per running container; spawn/cancel as containers come and go (use a `CancellationToken` or drop the task handle). Handle daemon-down gracefully (reconnect/backoff). |
| Terminal | crossterm via ratatui; raw mode + alt screen; `EventStream` in the select task | Restore terminal on panic (install a panic hook). Must work over SSH — braille/octant degrade to half-block/dot if the font lacks glyphs (offer a marker config). |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| Docker task ↔ main loop | `mpsc<DockerMsg>` | One-way, fire-and-forget; main loop is sole consumer. |
| Event task ↔ main loop | `mpsc<Event>` | Multiplexes input + tick + render. |
| world ↔ render3d | function call: `render(&world, &camera, vp)` | Read-only borrow; render produces a `Framebuffer`, never mutates world. |
| render3d ↔ ui | `Framebuffer` → `SceneShape` → `Painter::paint` | The single framebuffer→braille adapter; owns the coord-system mapping. |
| input ↔ app | `Action` enum | Decouples key bindings from state mutation; enables remapping/testing. |

## Library Decisions

- **Math: `glam`** (not nalgebra). glam is purpose-built for games/graphics, SIMD-backed `Mat4`/`Vec3`, simplest API (`Mat4::perspective_rh`, `Mat4::look_at_rh`), and fastest in mathbench for exactly these ops. nalgebra's general linear-algebra power is unnecessary and heavier here. (HIGH — mathbench-rs, glam docs.)
- **TUI: `ratatui` + `crossterm`** with the `Canvas` widget (Braille marker default; Octant/HalfBlock/Dot as configurable fallbacks for SSH/font-limited terminals). (HIGH — ratatui Canvas docs.)
- **Docker: `bollard`** — async streams for `stats`/`events`, async `list_*`/`inspect_*`. (HIGH — bollard docs.)
- **Runtime: `tokio`** (`mpsc`, `time::interval`, `select!`, `CancellationToken`). (HIGH.)
- **Stream combinators: `futures_util::TryStreamExt`** for consuming bollard streams. (HIGH — bollard examples.)
- **Reference implementation to study:** `rust-sloth` (triangle→charxel terminal rasterizer) for the rasterization-into-terminal approach. (MEDIUM — community project, validates feasibility.)

## Sources

### Primary (HIGH)
- [Ratatui — Full Async Events tutorial](https://ratatui.rs/tutorials/counter-async-app/full-async-events/) — tokio::select! event loop, Event enum, render-only-on-Render
- [Ratatui async-template (component architecture)](https://ratatui.github.io/async-template/02-structure.html) — App/Action/Tui/Components module structure
- [Ratatui Canvas widget](https://docs.rs/ratatui/latest/ratatui/widgets/canvas/struct.Canvas.html) — braille/octant/halfblock markers, x/y bounds
- [Ratatui Painter](https://docs.rs/ratatui/latest/ratatui/widgets/canvas/struct.Painter.html) — `paint(x,y,color)`, `get_point(fx,fy)` for framebuffer blit
- [bollard Docker struct](https://docs.rs/bollard/latest/bollard/struct.Docker.html) — `stats`/`events` stream signatures, `list_*`/`inspect_*`
- [mathbench-rs](https://github.com/bitshifter/mathbench-rs) + [glam](https://docs.rs/glam/) — math lib perf, Mat4 API
- [Fix Your Timestep! — Gaffer On Games](https://gafferongames.com/post/fix_your_timestep/) — fixed-step + interpolation decoupling

### Secondary (MEDIUM)
- [rust-sloth — 3D software rasterizer for the terminal](https://github.com/ecumene/rust-sloth) — triangle→charxel approach validating feasibility
- [bollard repo / examples](https://github.com/fussybeaver/bollard) — stats stream usage
- [Ratty coverage (The Register)](https://www.theregister.com/software/2026/05/11/ratty-terminal-emulator-brings-3d-graphics-to-the-command-line/5238299) — reference aesthetic (note: ratty uses Bevy + custom terminal, NOT a pure software braille rasterizer — 3dd's pure-software approach differs)

---
*Architecture research for: Rust TUI 3D Docker visualizer (3dd)*
*Researched: 2026-05-26*
