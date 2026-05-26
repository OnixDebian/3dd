# Project Research Summary

**Project:** 3dd (3D Docker)
**Domain:** Terminal (TUI) software-3D real-time visualizer over a local Docker daemon — Rust
**Researched:** 2026-05-26
**Confidence:** HIGH (engineering stack + data surface) / MEDIUM (3D-in-terminal legibility — the novel, unvalidated part)

## Executive Summary

3dd sits at the intersection of three areas: a Rust TUI app (ratatui + crossterm + tokio), a hand-rolled software-3D pipeline that rasterizes into terminal braille cells, and a read-only Docker observability layer (bollard streaming stats/events). The engineering side is well-trodden and low-risk — the crates are mature, the async-stream-meets-sync-render pattern is a solved problem, and the Docker data surface is fully documented. What is **not** solved and carries the real risk is *legibility*: whether a 3D scene of dozens of "blade-server" boxes, drawn in 2×4 braille dots under a moving camera, actually reads as a comprehensible 3D space rather than a field of noise.

All four research streams converge on one recommendation: **validate the rendering before building anything else.** The first deliverable should be a single aspect-correct, depth-cued cube that reads as 3D, animates smoothly, and idles at low CPU — proving out the project's four biggest threats at once (braille aspect ratio, occlusion/depth cues, render-loop CPU budget, panic-safe terminal restore). Only after that core is trustworthy should Docker data flow into it.

One important correction surfaced: **ratty (the cited aesthetic reference) is built on Bevy + GPU pixel readback inside a terminal emulator** — it is a *look*, not a technique 3dd can reuse. 3dd's pure-CPU braille rasterizer is a deliberately different and harder path, and it is exactly what gives 3dd its SSH/headless advantage over GPU tools. This must be understood going in.

## Key Findings

### Recommended Stack

Pure-Rust, async, no GPU. The TUI base (`ratatui`) already provides the braille machinery (`Canvas` widget + `Marker::Braille`, mapping a virtual coordinate space onto the 2×4 sub-pixel grid via `Painter::paint(x, y, color)`), so the braille packing does not need to be hand-rolled — only the 3D math feeding it does. The 3D pipeline (camera → projection → rasterize) is hand-built on `glam`. Docker access is `bollard` (async, Tokio), the only maintained client (`shiplift` is dead). See [STACK.md](STACK.md).

**Core technologies:**
- `ratatui` (~0.30): TUI framework + braille `Canvas` rendering — the linchpin render surface; verify exact version at roadmap-freeze (0.x, fast-moving)
- `crossterm` (~0.29): terminal backend + input (`poll()`/`read()`), panic-safe raw-mode restore
- `tokio` (~1.47): async runtime hosting the Docker stream tasks
- `bollard` (~0.21): Docker API client — streaming `stats()` + `events()` + list/inspect for containers/images/volumes/networks/ports
- `glam` (~0.33): matrix math (`look_at_rh` → `perspective_rh` → `project_point3`) — chosen over nalgebra for speed + graphics ergonomics
- `serde` + `toml`: config + theme definitions

**Highest technical risk crate:** the hand-rolled rasterizer on top of ratatui Canvas. Wireframe fits Canvas natively (low risk); solid/occluded faces need a custom z-buffered widget owning its own (2W)×(4H) pixel+depth buffer (separate, harder phase). Reference implementations to port: `rust-sloth`, `terminal3d`.

### Expected Features

3dd inverts the flat-tool model. lazydocker/ctop/oxker put *numbers in rows*; 3dd puts *state in space*: color = status, size = load, position = network, motion = liveness — with raw numbers demoted to an on-demand detail panel. The moat is legibility/aesthetics, not data coverage; 3dd intentionally shows **less data, more glanceably**. See [FEATURES.md](FEATURES.md).

**Must have (table stakes) — two axes:**
- *Data legibility:* status color, CPU/RAM-driven box size, name labels, live stats stream
- *3D legibility:* back-to-front occlusion (painter's sort — the single strongest depth cue; without it the scene reads flat), depth shading/fog, stable layout, smooth orbit motion
- Panic-safe terminal restore, resize handling, graceful "daemon not running" state

**Should have (differentiators):**
- Networks-as-floor-planes (containers grouped spatially by network) — the headline differentiator, but it reshapes the layout algorithm so it must be decided up front
- Autopilot orbit camera (screensaver feel) + manual explore (WASD/Tab/Enter detail panel)
- Runtime-switchable themes
- "Breathing" boxes via interpolated stats

**Defer (v1.x+):**
- Volume *byte-size* sizing — **Docker API does not expose volume size** (only a slow `docker system df -v` disk scan); size by a proxy or gate real sizing behind an explicit key
- Images-as-layered-stacks and ports-as-glowing-points (lower legibility payoff than containers/networks; defer if render-correctness is at risk)

**Anti-features (tie to Out of Scope):** container control, compose orchestration (grouping-only OK), remote/multi-host, in-scene log streaming, auto volume-size scan, historical graphs, GPU/web frontend, mouse-required navigation, photorealism, alerting.

### Architecture Approach

Single-consumer, message-passing core. Two async tokio tasks own the bollard streams and push `DockerMsg` into an `mpsc`; a `tokio::select!` task multiplexes input + frame-tick + render into a second `mpsc<Event>`. The **main loop is the only consumer and only writer of `App`/`World` state**, so `terminal.draw()` stays synchronous and never blocks on I/O — **channels, not `Arc<Mutex>`** (a lock held during a stats burst is the exact frame-hitch to avoid). See [ARCHITECTURE.md](ARCHITECTURE.md).

**Major components:**
1. **Docker data layer** — bollard streams → domain model (`docker/domain.rs`), bollard types isolated here so the visual core is testable without a daemon
2. **World model** — flat `Vec<Entity>` (no ECS/scene-graph; overkill at this scale) + layout placement (`world/layout.rs`) for rack/network positions
3. **Animation** — stats set `*_target`; `World::step(dt)` lerps `*_cur` toward targets (Gaffer fixed-timestep decoupling); this is what makes boxes "breathe"
4. **3D pipeline** — `render(&world, &camera, vp) -> Framebuffer`: glam transform → project → painter's-sorted rasterize into a pixel/depth buffer
5. **Render/UI** — custom ratatui `Shape`/widget blits the framebuffer to braille via `Painter::paint`; plus detail panel, status bar
6. **Input/camera controller** + **theme/config**

Stats update rate (~1-2 Hz) is fully decoupled from render rate (~30 fps steady tick), so slow Docker data still yields smooth motion.

### Critical Pitfalls

(Top items from [PITFALLS.md](PITFALLS.md) — 14 documented.)

1. **Legibility collapse (the #1 risk)** — braille artifacts + wrong aspect + no occlusion + moving labels + many containers + moving camera each individually fine but combine into noise. *Avoid:* prove a single readable cube before any Docker work; painter's-sort occlusion from day one.
2. **Docker CPU% delta gotcha** — the API returns cumulative nanosecond counters, not a percentage. Correct: `(cpu_delta / system_delta) * online_cpus * 100`. First stream sample has empty `precpu_stats` → garbage; `system_cpu_usage`/`online_cpus` can be 0/None → NaN box sizes. Memory must subtract cache. *Avoid:* implement the delta formula correctly with guards from the first commit that touches stats.
3. **Terminal cell aspect ratio (~1:2)** — uncorrected, cubes/circles look squashed. *Avoid:* a single explicit, config-exposed aspect-correction factor in the projection matrix — not scattered magic numbers.
4. **Render loop pegging a CPU core** — violates a hard project constraint. *Avoid:* `tokio::select!` frame tick + render-on-change + FPS cap; never a tight spin loop.
5. **Broken terminal on panic** — raw mode left enabled. *Avoid:* ratatui panic hooks (built in) installed at startup.

## Implications for Roadmap

Research converges on a **render-first** build order (do the risky, novel work before the well-understood Docker work). Depth is "quick" → lean. Suggested 5 phases (P5 can fold into P4 to compress to 4):

### Phase 1: Render Core & Legibility Spike
**Rationale:** The project's biggest risk is whether terminal-3D reads at all. De-risk it before investing in anything else.
**Delivers:** App skeleton (event loop, panic hooks, resize, clean shutdown) + a single aspect-correct, depth-shaded cube that orbits smoothly at low idle CPU.
**Addresses:** 3D-legibility table stakes (occlusion, depth cues, stable framing).
**Avoids:** Pitfalls #1 (legibility), #3 (aspect ratio), #4 (CPU peg), #5 (panic restore) — all four at once.

### Phase 2: 3D Pipeline on Synthetic Data
**Rationale:** Generalize the one cube into a full scene using fake/synthetic entities — unblocked by Docker, validates the pipeline and layout.
**Delivers:** Multiple boxes in a rack/datacenter layout, painter's-sorted occlusion, camera model (autopilot orbit), framebuffer→braille blit via custom `Shape`.
**Uses:** `glam`, ratatui `Canvas`/`Painter`. **Implements:** World model + 3D pipeline + layout components.
**Avoids:** Pitfall #1 at scale (does the scene still read with 50 boxes?).

### Phase 3: Docker Data Layer
**Rationale:** Only now feed real data into a proven renderer.
**Delivers:** bollard streams for containers + stats (correct CPU%/mem deltas) + events; domain model; daemon-down/permission-error handling; stream lifecycle as containers appear/disappear.
**Uses:** `bollard`, `tokio` mpsc single-consumer pattern. **Implements:** Docker data layer.
**Avoids:** Pitfall #2 (CPU% delta), stream-lifecycle leaks.

### Phase 4: Animation, Interaction & Full Entity Set
**Rationale:** Make it live and explorable; add the remaining entities once core data flows.
**Delivers:** Interpolated "breathing" boxes (target/current lerp); manual explore mode (WASD orbit, Tab select, Enter detail panel); networks-as-floor-planes; volumes/images/ports (proxy-sized where API can't supply real numbers).
**Implements:** Animation + input/camera controller + detail panel.
**Avoids:** Volume-size API trap (proxy/gated sizing), motion-sickness camera (eased, bounded orbit).

### Phase 5: Theming, Config & Validation Pass
**Rationale:** Polish and the themeable-palette requirement; final legibility/contrast validation across terminals.
**Delivers:** `serde`/`toml` config, runtime theme switching (neon / terminal-green / Notion-soft), color-depth fallback (truecolor → 256 → 16), aspect-correction knob exposed.
**Avoids:** Theme contrast failures, non-truecolor terminal breakage.

### Phase Ordering Rationale
- **Render-first, Docker-second** is the unanimous recommendation: the novel risk (legibility) is front-loaded, and Phases 1–2 need no daemon, so they can't be blocked by Docker plumbing.
- The themeable-palette abstraction must exist before any hardcoded status color — kept as P5 but the *color abstraction* should be introduced in P1 to avoid rework (flagged for planning).
- Networks-as-floors reshapes layout, so the layout algorithm in P2 must be designed network-aware even though networks land in P4.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 1:** the framebuffer→braille coordinate mapping (Canvas math-coords bottom-left vs grid top-left, sizing to 2×4 sub-pixels/cell) needs a small spike; the exact aspect-correction constant is font/terminal-dependent — ship as a config knob, don't hardcode.
- **Phase 2:** the rack-vs-network-floor **layout algorithm** is load-bearing and unresolved — recommend a focused design spike before planning P2.
- **Phase 4:** label occlusion in braille (overlapping moving billboards) is the hardest text problem — MEDIUM confidence it's tractable at v1 quality; prototype early.

Phases with standard patterns (skip research-phase):
- **Phase 3:** Docker/bollard integration is well-documented; the only gotcha (CPU% delta) is already captured in PITFALLS.md.
- **Phase 5:** serde/toml config + theming are standard.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Versions verified on crates.io/docs.rs; rasterizer specifics MEDIUM (hand-rolled, no blessed crate) |
| Features | HIGH (data) / MEDIUM (3D legibility) | Data surface from real tools; 3D-metaphor legibility is novel/unvalidated |
| Architecture | HIGH | Async/sync integration + pipeline are established patterns; layout algorithm MEDIUM |
| Pitfalls | HIGH | Three load-bearing pitfalls verified against primary sources (moby issue, bollard docs) |

**Overall confidence:** HIGH on "can it be built"; MEDIUM on "will it look good" — which is why P1 is a legibility spike.

### Gaps to Address
- **Layout algorithm (rack grid vs network-grouped floors):** decide during P2 planning; design network-aware from the start.
- **Aspect-correction constant:** font/terminal-dependent — ship as config, validate empirically in P1.
- **Volume size:** not in Docker API — decide proxy metric vs gated on-demand scan in P4 planning.
- **MVP bundling tension:** PROJECT.md wants the full entity set in v1; research recommends render-correctness first, volumes/images/ports deferred to P4/v1.x. Reconcile in `/gsd:define-requirements`.
- **Painter's-sort vs z-buffer:** boxes rarely interpenetrate, so painter's likely suffices; settle in the P1 spike.

## Sources

### Primary (HIGH confidence)
- [ratatui — docs.rs](https://docs.rs/ratatui/) + [Canvas](https://docs.rs/ratatui/latest/ratatui/widgets/canvas/struct.Canvas.html) / [Painter](https://docs.rs/ratatui/latest/ratatui/widgets/canvas/struct.Painter.html) — braille render surface
- [bollard — docs.rs](https://docs.rs/bollard/) + [Stats](https://docs.rs/bollard/latest/bollard/container/struct.Stats.html) — streaming API + CPU% fields
- [moby/moby #29306](https://github.com/moby/moby/issues/29306) — Docker stats CPU% delta formula
- [crossterm](https://docs.rs/crate/crossterm/latest), [tokio mpsc](https://docs.rs/tokio/latest/tokio/sync/mpsc/), [glam](https://docs.rs/glam/)
- [Ratatui Full Async Events](https://ratatui.rs/tutorials/counter-async-app/full-async-events/) + [panic hooks](https://ratatui.rs/recipes/apps/panic-hooks/)

### Secondary (MEDIUM confidence)
- [orhun/ratty](https://github.com/orhun/ratty) + [intro blog](https://blog.orhun.dev/introducing-ratty/) — aesthetic reference (GPU/Bevy, NOT the technique)
- [rust-sloth](https://github.com/ecumene/rust-sloth), [terminal3d](https://crates.io/crates/terminal3d), [drawille](https://docs.rs/drawille/) — terminal rasterizer references
- [lazydocker](https://lazydocker.com/), [oxker](https://github.com/mrjackwills/oxker), [dive](https://github.com/wagoodman/dive), [ctop](https://kx.cloudingenium.com/en/ctop-container-metrics-monitor-docker-podman-terminal-guide/) — feature surface
- [Terminal graphics with Braille](https://danieledapo.github.io/post/terminal-graphics-braille/), [Fix Your Timestep!](https://gafferongames.com/post/fix_your_timestep/)
- [Docker system df](https://docs.docker.com/reference/cli/docker/system/df/) — volume-size limitation

### Tertiary (LOW confidence)
- [Depth perception in computer graphics (Cambridge)](https://www.cl.cam.ac.uk/techreports/UCAM-CL-TR-546.pdf) — occlusion as strongest depth cue (general, applied by inference)

---
*Research completed: 2026-05-26*
*Ready for roadmap: yes*
