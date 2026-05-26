# Stack Research

**Domain:** Terminal (TUI) real-time software-3D visualizer for the local Docker daemon, written in Rust
**Researched:** 2026-05-26
**Confidence:** HIGH (core TUI/Docker/async/math); MEDIUM (software-3D rasterizer approach — hand-rolled, not a single blessed crate)

## Summary (read first)

The entire stack is buildable from mature, current crates with **no GPU dependency**, satisfying the hard TUI-only constraint. The architecture is a classic split:

- **Sync render thread** drives `ratatui` at a fixed framerate, drawing the 3D scene into the `Canvas` widget with `Marker::Braille` (native 2×4 subpixel grid).
- **Async tokio tasks** own `bollard` streams (stats/events) and push deltas over `tokio::sync::mpsc` channels into shared app state read by the render loop.
- **The 3D pipeline is hand-rolled** (transform → project → rasterize to braille dots) on top of `glam` for matrix math. This is the norm for terminal-3D; there is no drop-in "software-3D-into-ratatui" crate. **This is the linchpin and the highest technical risk.**

**Critical correction re: ratty:** Despite being the named aesthetic reference, `orhun/ratty` is **NOT** a model to copy structurally. Ratty renders 3D with **Bevy + Vello/Parley on the GPU** and reads pixels back to the terminal. That violates this project's TUI-only / SSH-friendly / no-GPU constraint. Use ratty for *visual inspiration only*; build the renderer the way `rust-sloth` / `terminal3d` / `drawille` do — pure CPU.

**Primary recommendation:** `ratatui 0.30` (Canvas + `Marker::Braille`) + `glam 0.33` (math) + hand-rolled rasterizer + `bollard 0.21` (Docker) + `tokio 1.47` (async) + `crossterm 0.29` (backend/input) + `serde`/`toml` (themes).

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| **ratatui** | 0.30.0 | TUI framework, layout, double-buffered rendering, widgets | De-facto standard Rust TUI (succeeds `tui-rs`). Ships the **`Canvas` widget** whose `Context` exposes a per-cell grid that, with `Marker::Braille`, gives a **2×4 subpixel** drawing surface — exactly the braille trick this project needs, with line/point/shape primitives and per-dot color. No need to reinvent the terminal diff/flush layer. |
| **crossterm** | 0.29.0 | Terminal backend + raw mode + keyboard/resize input | Cross-platform, the default ratatui backend (`crossterm_0_29` feature). Provides the WASD/Tab/Enter key events for manual camera mode. Works fine over SSH. Has `event-stream` (async `EventStream`) but for this app **synchronous `poll()`/`read()` on the render thread is simpler** (see Architecture). |
| **glam** | 0.33.0 | 3D math: `Vec3`, `Mat4`, perspective + look-at, projection | The fastest, simplest game-graphics math crate (SIMD f32). Has everything the camera/projection pipeline needs out of the box: `Mat4::perspective_rh`, `Mat4::look_at_rh`, `Mat4::project_point3` (does the perspective divide for you). Smaller/simpler API than nalgebra for pure 3D rendering. |
| **bollard** | 0.21.0 | Async Docker daemon API client | The actively-maintained Rust Docker client. Confirmed support for **streaming stats** (`stats` → `Stream<ContainerStatsResponse>`, `StatsOptionsBuilder::stream(true)`), **streaming events** (`Stream<EventMessage>`), and list/inspect for containers, images, volumes, networks, and ports (container inspect). Auto-discovers the local unix socket; also supports Podman. Built on hyper 1.x + tokio 1.x. |
| **tokio** | 1.47.x | Async runtime for bollard streams + channels | Required by bollard (`^1.47`). Use the multi-thread or current-thread runtime to host the Docker stream tasks; `tokio::sync::mpsc` bridges async producers → sync render consumer. The ecosystem standard. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| **serde** | 1.x (`derive` feature) | Config/theme (de)serialization | Always — derive on `Config`/`Theme` structs. |
| **toml** | 0.8.x / 0.9.x | Parse the `~/.config/3dd/config.toml` theme & settings file | Always — TOML is the idiomatic Rust config format; pairs with serde. |
| **color-eyre** *(or `anyhow`)* | 0.6.x / 1.x | Ergonomic error handling + nice panic/error reports in a TUI | Recommended — ratatui's templates use `color-eyre`; restores the terminal on panic. |
| **dirs** *(or `directories`)* | 5.x / 6.x | Locate `$XDG_CONFIG_HOME` for the config file | When loading themes from a standard config path. |
| **palette** | 0.7.x | Color-space math (HSL/Lab interpolation) for smooth theme gradients & status colors | Optional — only if you want perceptually-smooth color ramps for CPU/RAM "heat" rather than simple RGB lerp. |
| **futures-util** | 0.3.x | `StreamExt` (`.next()`) for consuming bollard streams | Always (transitively present; import for stream combinators). |
| **clap** | 4.x | CLI args (`--theme`, `--docker-host`, etc.) | Optional but cheap; nice for SSH usage. |

### Reference Implementations (study these, don't depend on them)

| Project | What to learn from it |
|---------|------------------------|
| **drawille** (docs.rs/drawille) | The canonical Rust braille-canvas port. Shows the `U+2800 + bitmask` dot-packing. If ratatui's Canvas ever proves too limiting, this is the fallback drawing primitive. |
| **rust-sloth** (ecumene/rust-sloth) | A working terminal **triangle software rasterizer** (uses `termion` + `tobj` + `nalgebra`). Reference for the vertex→screen pipeline and triangle fill. |
| **terminal3d** (crates.io/terminal3d) | Renders 3D models with braille AND block markers, wireframe + vertex modes. Closest existing analog to the desired look. |
| **rendersloth** (lib.rs/rendersloth) | "charxel" rasterizer (character + color per cell) — reference for combining color with subpixel coverage. |

## Installation

```bash
# Core
cargo add ratatui                 # 0.30.x (pulls crossterm_0_29 backend by default)
cargo add crossterm               # 0.29.x (explicit, for event types/keycodes)
cargo add glam                    # 0.33.x
cargo add bollard                 # 0.21.x
cargo add tokio --features full   # 1.47.x

# Supporting
cargo add serde --features derive # 1.x
cargo add toml                    # 0.8/0.9
cargo add futures-util            # 0.3.x
cargo add color-eyre              # 0.6.x
cargo add dirs                    # 5/6

# Optional
cargo add palette                 # 0.7.x  (smooth color ramps)
cargo add clap --features derive  # 4.x
```

> Verify exact patch versions at publish time with `cargo add` / crates.io; versions above are confirmed current as of 2026-05.

## How you actually rasterize 3D into braille cells (the linchpin)

This is the core technical answer the project hinges on. There is **no single crate** that does "feed me triangles, render into a ratatui widget." You compose two well-understood pieces:

**1. The braille subpixel trick (the "framebuffer").**
The Unicode Braille Patterns block starts at `U+2800`. Each glyph encodes up to 8 dots in a 4-row × 2-col grid; each dot = one bit of an 8-bit offset, so `char = char::from_u32(0x2800 + bitmask)`. Two glyphs can be merged with bitwise OR. So one terminal cell = an 8-pixel (2×4) mini-framebuffer. A terminal of `W×H` cells therefore gives you a `(2W)×(4H)` pixel canvas.
**You do not implement this yourself** — `ratatui`'s `Canvas` widget with `Marker::Braille` already maps a virtual `(x, y)` coordinate space onto this dot grid and handles the glyph packing + per-cell coloring. (`drawille` is the standalone equivalent if you bypass ratatui.)

**2. The 3D pipeline (hand-rolled, on glam).** Per frame:
1. **Model → world:** place each container/rack via a model `Mat4` (translation/scale from CPU/RAM).
2. **World → view:** `Mat4::look_at_rh(eye, target, up)` — `eye`/`target` come from the active camera (autopilot orbit angle, or WASD-driven free-fly position).
3. **View → clip → NDC:** `Mat4::perspective_rh(fov, aspect, near, far)`; transform points with `proj_view.project_point3(p)` which **does the perspective divide** for you, yielding NDC in roughly `[-1, 1]`.
4. **NDC → screen:** map NDC x/y to the Canvas coordinate range (i.e. `(2W)×(4H)` braille pixels), flipping Y.
5. **Rasterize:** for the datacenter/rack metaphor, **wireframe-first is the pragmatic v1** — draw box edges as `Line`s via `ctx.draw(&Line {...})` in braille; points/ports as single dots. Filled faces require a triangle rasterizer + a **per-pixel depth buffer (z-buffer)** for correct occlusion — this is where `rust-sloth` is the reference. Color comes from container status (theme palette); intensity/coverage can encode CPU/RAM.

**Risk notes:**
- ratatui's `Canvas` is designed for shapes (lines, points, rectangles, maps), not z-buffered triangle fills. **Wireframe + depth-sorted edges fit Canvas naturally; solid shaded faces likely need a custom widget** that owns its own `(2W)×(4H)` pixel+depth buffer and emits braille glyphs into the ratatui `Buffer` directly (the `drawille` approach). Plan for a custom widget as soon as you want filled, occluded surfaces.
- Performance is fine for hundreds of boxes at 30–60fps on CPU; thousands of shaded triangles per frame is where you'd feel it. The rack metaphor (tens–hundreds of boxes) is comfortably in range.

## Async Docker stream ↔ sync render loop (the integration pattern)

```
[tokio runtime]                         [render thread / main]
 ┌─ stats stream task  ──┐               ┌──────────────────────────┐
 │  bollard.stats(...)   │  mpsc::Sender │  loop {                  │
 │  .next().await ───────┼──────────────▶│   while let Ok(m)=rx.try_recv(){apply(m)}
 ├─ events stream task ──┤               │   if poll(16ms){handle_key(read())}
 │  bollard.events(...)  │               │   update_camera(dt)      │
 ├─ periodic list task ──┘               │   terminal.draw(|f| render_scene(f, &state))
 └───────────────────────                │  }                       │
                                         └──────────────────────────┘
```

- **Producers (async):** one tokio task per concern — a `stats` stream per running container (or a fan-in task), an `events` stream, and a low-frequency `list_containers/images/volumes/networks` poller. Each sends typed messages over a `tokio::sync::mpsc::unbounded_channel`.
- **Consumer (sync):** the render loop drains the channel **non-blocking** (`try_recv`) each frame, applies deltas to a shared scene/state struct, advances the camera by `dt`, then `terminal.draw(...)`. Input via `crossterm::event::poll(timeout)` + `read()` keeps the loop at a steady tick without an async event stream.
- **Why not `crossterm` async `EventStream`?** You *can* go fully async (select! over input + docker + a frame timer), but a sync render thread + mpsc bridge is simpler to reason about, avoids `Send`/`'static` friction with ratatui's `Frame`, and is the common ratatui pattern. Choose async-everything only if you prefer a single `tokio::select!` loop.

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| glam | **nalgebra** (+ `nalgebra-glm`) | If you later need generic/dynamically-sized linear algebra, stronger typing, or heavier geometry math. For pure render-pipeline 3D, glam is simpler and faster. `rust-sloth` uses nalgebra — fine, just heavier. |
| ratatui Canvas + Braille | **drawille** (standalone) | If you bypass ratatui for the 3D surface and want a raw braille framebuffer you fully control. Best as the internals of a *custom ratatui widget*, not as a replacement for ratatui. |
| Wireframe rasterizer (DIY) | **terminal3d / rendersloth as a lib** | Study/borrow their rasterizer code; neither is a clean embeddable dependency for a live, stateful app, so expect to vendor ideas rather than `cargo add` them. |
| sync render + mpsc | Full `tokio::select!` async loop + `crossterm` `EventStream` | If the team prefers all-async and is comfortable threading the ratatui draw through it. |
| color-eyre | **anyhow** | If you don't want the extra report formatting; both fine. |
| Marker::Braille | **Marker::HalfBlock** / `Marker::Octant` | HalfBlock gives true 24-bit-per-subpixel color at lower spatial res (2×2-ish); good fallback for terminals/fonts lacking braille, or when color fidelity matters more than resolution. Worth exposing as a runtime render-mode toggle. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| **Bevy / Vello / Parley / any wgpu path (the ratty architecture)** | Requires a GPU + windowing/readback; breaks the hard TUI-only & SSH constraints. Ratty is the *look* reference, not the *tech* reference. | Pure-CPU pipeline: ratatui Canvas/`Marker::Braille` + glam + DIY rasterizer. |
| **shiplift** | Unmaintained/deprecated Rust Docker client. | bollard 0.21 |
| **dockworker / docker-api** | Less maintained / smaller; bollard is the streaming-first, actively-maintained standard with confirmed stats+events streaming. | bollard 0.21 |
| **tui-rs (`tui` crate)** | The original TUI crate; abandoned. ratatui is its maintained fork/successor. | ratatui 0.30 |
| **termion backend** | Linux/Unix-only, fewer features, less common with ratatui than crossterm. | crossterm 0.29 (ratatui default) |
| **Hand-packing braille codepoints yourself** | Error-prone, and ratatui's Canvas already does the `U+2800 + bitmask` mapping with coloring. | ratatui Canvas `Marker::Braille` (drop to `drawille`-style only inside a custom widget if needed) |
| **Blocking/sync Docker HTTP calls in the render loop** | Stalls frames; stats/events are inherently streaming. | tokio tasks + bollard streams + mpsc to the render thread |

## Stack Patterns by Variant

**If v1 = wireframe rack metaphor (recommended start):**
- ratatui `Canvas` + `Marker::Braille`, `ctx.draw(&Line)` for box edges, dots for ports.
- glam for transform/projection; depth-sort edges back-to-front (painter's algorithm) — no full z-buffer needed yet.
- Lowest risk; ships the aesthetic fast.

**If you later want solid, occluded, shaded surfaces:**
- Build a **custom ratatui widget** owning a `(2W)×(4H)` color + **z-buffer**; rasterize triangles (port `rust-sloth`'s fill), emit braille glyphs into the `Buffer`.
- glam stays; add per-face lighting/shading. Higher complexity; do it as a phase, not v1.

**If braille is unsupported on a target terminal/font:**
- Expose a runtime render-mode toggle to `Marker::HalfBlock` (also gives richer color) — wire it to the same theme system.

## Version Compatibility

| Package | Compatible With | Notes |
|---------|-----------------|-------|
| ratatui 0.30 | crossterm 0.29 (`crossterm_0_29` feature, default) | 0.30 split into workspace crates (`ratatui-core`, `ratatui-crossterm`, …); the umbrella `ratatui` crate is what apps use. Can also select `crossterm_0_28`. |
| bollard 0.21 | tokio ^1.47, hyper ^1.3 | Pin tokio to ≥1.47 to match bollard's lower bound. |
| glam 0.33 | — | Standalone; `Mat4::project_point3` / `perspective_rh` / `look_at_rh` all present. |
| serde 1 | toml 0.8/0.9 | Standard pairing; enable serde `derive`. |
| crossterm 0.29 | tokio (via `event-stream` feature) | Only needed if you opt into async `EventStream`; not required for the sync-loop pattern. |

## Sources

- https://crates.io/crates/ratatui & https://docs.rs/ratatui/latest/ratatui/ — confirmed **ratatui 0.30.0**, workspace split, crossterm 0.28/0.29 backend features — HIGH
- https://docs.rs/ratatui/latest/ratatui/widgets/canvas/struct.Canvas.html & https://ratatui.rs/examples/widgets/canvas/ — Canvas widget, `Marker::Braille` (2×4), Octant, HalfBlock; `ctx.draw(&Line/&Points)` — HIGH
- https://crates.io/crates/bollard & https://docs.rs/bollard/latest/bollard/ — **bollard 0.21.0**, streaming stats/events, container/image/volume/network APIs, tokio ^1.47 / hyper ^1.3 — HIGH
- https://docs.rs/glam/latest/glam/ & https://docs.rs/glam/latest/glam/f32/struct.Mat4.html — **glam 0.33.0**, `Mat4` perspective_rh/look_at_rh/project_point3 — HIGH
- https://docs.rs/crossterm/latest & https://docs.rs/crate/crossterm/latest — **crossterm 0.29.0**, `poll`/`read`, `EventStream` (event-stream feature) — HIGH
- https://docs.rs/tokio/latest/tokio/sync/mpsc/ — tokio mpsc bounded/unbounded, runtime-agnostic channel — HIGH
- https://github.com/orhun/ratty — ratty uses **Bevy + Vello/Parley GPU bridge** (NOT CPU braille) — flagged as architecture anti-pattern for this project — HIGH
- https://danieledapo.github.io/post/terminal-graphics-braille/ — braille `U+2800 + bitmask` dot-packing + OR-merge formula — HIGH
- https://github.com/ecumene/rust-sloth — terminal triangle software rasterizer (termion + nalgebra + tobj) — MEDIUM (reference impl, structure inferred from README)
- https://docs.rs/drawille/ , https://crates.io/crates/terminal3d , https://lib.rs/crates/rendersloth — existing Rust terminal-3D/braille renderers — MEDIUM
- https://github.com/bitshifter/mathbench-rs — glam vs nalgebra performance (glam faster via SIMD) — HIGH

---
*Stack research for: terminal 3D Docker visualizer (Rust, TUI-only)*
*Researched: 2026-05-26*
*Valid until: ~2026-07 (fast-moving: ratatui 0.x, bollard 0.x — re-verify versions before roadmap freeze)*
