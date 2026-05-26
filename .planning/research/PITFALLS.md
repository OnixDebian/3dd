# Pitfalls Research

**Domain:** Terminal-based software 3D rendering + real-time Docker stats streaming (Rust TUI: ratatui + bollard + glam + crossterm + tokio)
**Researched:** 2026-05-26
**Confidence:** HIGH on the three load-bearing pitfalls (Docker CPU% delta, cell aspect ratio, render-loop CPU); MEDIUM/HIGH elsewhere.

## THE SINGLE BIGGEST RISK

**The whole project lives or dies on one perceptual question: "Does the 3D scene read clearly, or is it a mess of dots?"** Every other pitfall here is a solvable engineering problem with known fixes. Legibility is a design problem that compounds: braille rasterization artifacts + wrong aspect ratio + no real occlusion + moving labels + 50 containers + a moving camera can *each* be individually "correct" and still combine into noise. PROJECT.md states the core value explicitly: "If everything else fails, the scene must look good and read clearly."

**Implication for the roadmap:** Do NOT defer legibility to a polish phase. The very first phase must render *one box, correctly aspect-corrected, with depth cues, that reads as a 3D cube* before any Docker integration. If a single cube doesn't read as 3D in a terminal, 50 containers never will. Treat the rasterizer's perceptual quality as the riskiest assumption to validate first (spike it).

The CPU%-delta gotcha and render-loop CPU are the highest-impact *correctness/constraint* risks; legibility is the highest-impact *product* risk.

---

## Critical Pitfalls

### Pitfall 1: Docker stats CPU% computed without the delta (the classic gotcha)

**What goes wrong:**
The Docker stats API does NOT return a CPU percentage. It returns raw cumulative nanosecond counters. Developers read `cpu_stats.cpu_usage.total_usage` and display it directly (a meaningless ever-growing number), or divide by the wrong denominator, getting values that are wildly wrong (e.g. >100% always, or always ~0%). The correct percentage requires a *delta between two consecutive samples*:

```
cpu_delta    = cpu_stats.cpu_usage.total_usage - precpu_stats.cpu_usage.total_usage
system_delta = cpu_stats.system_cpu_usage      - precpu_stats.system_cpu_usage
online_cpus  = cpu_stats.online_cpus  (fallback: len(percpu_usage) if 0/None)
cpu_pct      = (cpu_delta / system_delta) * online_cpus * 100.0
```

Additional traps inside this trap:
- **First sample is garbage.** On the first frame of a stream (or any `one_shot`/non-streaming call), `precpu_stats` is zero/empty, so `system_delta` is huge and the percentage is near-zero or nonsensical. You must discard or specially-handle the first reading per container.
- **`system_cpu_usage` and `online_cpus` are `Option`/can be 0** (notably on Windows daemons, cgroup v1 vs v2 differences). Dividing by zero → NaN/Inf → box size becomes NaN → renderer produces garbage or panics.
- **Memory has its own gotcha:** "used" memory is `memory_stats.usage - memory_stats.stats["cache"]` (or `inactive_file` on cgroup v2). Naive `usage` over-reports because it includes page cache.

**Why it happens:**
The streaming JSON *looks* complete — there's a field called `cpu_usage`, so people assume it's usable as-is. The need for two samples is non-obvious and only documented in moby issue #29306 and the CLI source, not prominently in client libraries.

**How to avoid:**
- Implement the delta formula exactly as above. Use bollard's streaming `stats(name, StatsOptions { stream: true, ... })` so each emitted `Stats` already carries `precpu_stats` from the previous sample inside the stream — bollard/Docker populates precpu for you on a *stream*; don't recompute deltas yourself across your own polling.
- Guard every division: if `system_delta <= 0` or `online_cpus == 0`, emit `0.0`, not NaN. Clamp result to `[0, online_cpus*100]`.
- Mark the first sample per container as "warming up" and don't size the box from it.
- For memory, subtract cache; clamp to `[0, limit]`.

**Warning signs:**
CPU% that only ever climbs, is always 0, always >>100%, or boxes that flash to enormous/zero size on the first frame a container appears. Any `NaN`/`inf` in box dimensions.

**Phase to address:** Phase 2 (Docker data layer) — write the stats normalizer as the first thing in that phase with unit tests on synthetic before/after samples.

---

### Pitfall 2: Terminal cell aspect ratio not corrected — everything looks squashed

**What goes wrong:**
Terminal character cells are ~1:2 (twice as tall as wide). A braille cell packs a 2×4 dot grid (2 dots wide, 4 tall). If you treat dot coordinates as a square pixel grid and project 3D → 2D with a 1:1 mapping, cubes render as flattened slabs, "orbits" look elliptical, and the datacenter rack looks pancaked. This is the #1 cosmetic giveaway of an amateur terminal-3D renderer.

**Why it happens:**
The math is "correct" in screen-space pixels; the developer forgets that a braille dot is not square on screen. With a 2×4 dot cell inside a 1:2 character cell, the *effective* on-screen dot spacing is roughly square horizontally vs vertically only by coincidence — but it depends on the font and terminal, so it is never exactly right unless you make the correction an explicit, configurable factor.

**How to avoid:**
- Introduce a single explicit `cell_aspect` / `pixel_aspect` scale factor in the projection matrix (multiply X or Y by it). Derive default from: char cell ≈ 1:2, braille subdivisions 2×4 → net dot aspect ≈ (1/2 width)/(1/4 height) of the cell. Start with a correction near `2.0` on the horizontal axis and **make it a config/theme knob** so users on odd fonts can tune it.
- Validate with a known shape: render a wireframe sphere/cube and confirm it looks round/cubic, not egg-shaped, before trusting any scene.

**Warning signs:**
Spheres look like eggs, cubes look like bricks, autopilot orbit looks like an oval, vertical motion feels "faster" than horizontal.

**Phase to address:** Phase 1 (rasterizer spike). This must be correct in the very first single-cube render; it is the cheapest pitfall to fix early and the most expensive to retrofit after scene code assumes square pixels.

---

### Pitfall 3: Tight render loop pegs a CPU core (explicit constraint violation)

**What goes wrong:**
A naive `loop { render(); }` with no frame pacing spins at thousands of FPS, redrawing an identical scene, pinning a core at 100%. PROJECT.md makes "must NOT peg CPU" an explicit hard constraint, so this is a *requirement failure*, not just inefficiency. It's especially bad because the tool is pitched as a passive screensaver — running it idle on a laptop must not melt the battery.

**Why it happens:**
TUI tutorials show `loop { terminal.draw(); handle_events(); }` which busy-waits. Combined with a full-screen braille framebuffer (a large per-frame rasterization cost), each iteration is expensive *and* runs as fast as possible.

**How to avoid:**
- **Cap the frame rate** (e.g. 30 FPS for autopilot, lower for idle). Use a `tokio::time::interval` / `tokio::select!` over (frame tick, input event, stats update) so the loop sleeps between frames instead of spinning.
- **Render-on-change:** when the camera is static (manual mode, no input) and no stats changed, skip re-rasterizing. Only autopilot's moving camera needs continuous frames.
- **Decouple stats cadence from render cadence:** Docker stats arrive ~1/sec per container; do not re-poll per frame. Interpolate ("breathing") between samples on the render side.
- Add a startup self-check / soak target: idle autopilot should sit at single-digit % CPU on one core.

**Warning signs:**
Fan spins up immediately on launch; `top` shows 100% of a core; battery drains fast; the app is hot even when the scene isn't visibly moving.

**Phase to address:** Phase 1 (event/render loop architecture) — the loop must be tick-based from the start; retrofitting frame pacing into a busy loop touches everything.

---

### Pitfall 4: Depth/occlusion done wrong (or not at all) → unreadable overlap

**What goes wrong:**
Without depth handling, far blade-servers draw over near ones, edges of overlapping racks interleave, and the scene becomes ambiguous spaghetti. A full per-dot z-buffer at braille resolution is also expensive and can hurt Pitfall 3.

**Why it happens:**
Software 3D in a terminal tempts you to skip the z-buffer ("it's just wireframes"). But wireframes with no occlusion are the *least* readable — every edge shows through everything.

**How to avoid:**
- For the datacenter metaphor, **painter's algorithm** (sort entities back-to-front by camera distance and draw in order) is usually sufficient and cheap, because blade-servers are convex-ish boxes that rarely interpenetrate. Avoid a per-pixel z-buffer unless overlap artifacts demand it.
- Add explicit depth *cues* beyond occlusion: dim/desaturate distant boxes (fog), thinner lines far away, brightness falloff. These do more for legibility than perfect occlusion.
- Prefer solid/face-shaded boxes over pure wireframe for the main bodies; reserve wireframe for connections (networks).

**Warning signs:**
Edges of a far rack visibly pass through a near one; you can't tell which container is in front; scene reads as flat overlapping outlines.

**Phase to address:** Phase 1 (rasterizer) for painter's-sort + fog; refine in Phase 3 (scene/aesthetics).

---

### Pitfall 5: Labels in a moving 3D scene that are unreadable or jittering

**What goes wrong:**
Container names projected onto moving 3D boxes jitter, overlap each other, flip behind the camera, or render at sub-cell positions that snap around every frame. With 50 containers and an orbiting camera, labels become an unreadable swarm.

**Why it happens:**
Text lives on a character grid (1 char = 1 cell), but 3D projection produces fractional sub-cell positions and continuous motion. Naively billboarding every label every frame produces chaos.

**How to avoid:**
- **Don't label everything.** Label only the focused/selected container (manual mode) and maybe the nearest N in autopilot. Show full names in a 2D side/detail panel, not in the 3D space.
- Snap label anchor to the cell grid and apply hysteresis (don't move a label until its anchor moves >1 cell) to kill jitter.
- Cull labels that are behind the camera or beyond a distance; collision-resolve overlapping labels by dropping the farther one.
- Consider drawing labels in a separate 2D overlay pass (ratatui text widgets) on top of the braille canvas, anchored to projected positions, rather than rasterizing text into braille dots.

**Warning signs:**
Label text shivers/jitters frame-to-frame; names overlap into mush; labels appear for containers behind the camera; can't read any single name during orbit.

**Phase to address:** Phase 3 (scene composition / UX).

---

### Pitfall 6: Color resolution assumed truecolor; breaks on 256/16-color terminals & SSH

**What goes wrong:**
Themes authored in 24-bit truecolor (cyberpunk neon gradients) collapse on a 256-color or 16-color terminal — fog/brightness gradients band into a few flat colors, neon-on-dark becomes invisible, and over some SSH/tmux sessions colors are wrong entirely. Status colors (running/stopped/crashed) that rely on subtle hues become indistinguishable.

**Why it happens:**
Developer's own terminal is truecolor; `COLORTERM=truecolor` is assumed. tmux/screen and older SSH targets frequently downgrade to 256 or 16 colors.

**How to avoid:**
- Detect color depth at startup (`COLORTERM`, terminfo) and pick/quantize the palette accordingly; ratatui/crossterm can emit truecolor but the terminal may ignore it.
- Make status *distinguishable by more than hue*: vary brightness and, ideally, a glyph/shape or border so red/green colorblind and 16-color users can still tell states apart.
- Author themes with sufficient contrast; test each theme on a 256-color and a 16-color profile. Provide a high-contrast fallback theme.

**Warning signs:**
Gradients band into stripes; "neon" looks gray; two statuses look identical over SSH/tmux; user reports "I can't read it on my server."

**Phase to address:** Phase 3 (theming) — but the color *abstraction* (don't hardcode RGB at draw sites) belongs in Phase 1's renderer.

---

### Pitfall 7: Containers appearing/disappearing mid-stream crash or leak

**What goes wrong:**
A container is `docker rm`'d while its stats stream is open → the stream errors/ends; if unhandled, the task panics or the whole runtime stalls. New containers started after launch never appear because you only enumerated once at startup. Old containers' boxes linger forever (leak) or vanish abruptly (jarring).

**Why it happens:**
Treating the container set as static (enumerate once) and treating each stats stream as infallible. Docker is a moving target.

**How to avoid:**
- Subscribe to the **Docker events stream** (`events()`) for create/start/die/destroy and reconcile the scene's entity set, instead of (or in addition to) periodic `list_containers`.
- Each per-container stats stream runs in its own task; on stream end/error, remove the container gracefully (fade out / shrink) rather than panicking. Use a join/abort handle keyed by container ID so removed containers' tasks are cancelled.
- Animate enter/exit (fade/scale) so the scene doesn't pop.

**Warning signs:**
App panics when you stop a container; new containers don't show up; dead containers stay forever; orphaned tokio tasks accumulate.

**Phase to address:** Phase 2 (Docker data layer / reconciliation).

---

### Pitfall 8: Blocking the async runtime with rasterization or sync I/O

**What goes wrong:**
The CPU-heavy 3D rasterization (or any `std::io` blocking write) runs directly on a tokio worker, starving the async tasks that read Docker streams → stats stall, UI hitches, streams back up. Conversely, `tokio` is used where it isn't needed and adds complexity.

**Why it happens:**
Everything-is-async cargo-culting, or the opposite — doing blocking terminal writes inside an async context.

**How to avoid:**
- Keep the render/rasterize work on a dedicated thread or the main thread driven by a frame tick; keep tokio for I/O (bollard streams, events). Communicate via channels (`tokio::sync::mpsc` / `watch`) — stats tasks push samples, render loop reads latest.
- Never do long CPU work inside an `async fn` on a runtime worker without `spawn_blocking`. Terminal writes should be batched per frame (one flush), not interleaved with async awaits.

**Warning signs:**
Stats updates freeze while camera moves; input lag; streams deliver bursts after pauses; profiler shows render work on tokio workers.

**Phase to address:** Phase 1 (loop/threading architecture) + Phase 2 (stats task wiring).

---

### Pitfall 9: Daemon down / socket permission errors handled as a crash

**What goes wrong:**
Docker isn't running, or the user isn't in the `docker` group → connecting to `/var/run/docker.sock` fails with a permission/connection error. A naive `.unwrap()` panics *while the terminal is in raw mode/alternate screen*, leaving the user with a broken, unreadable terminal and no useful message.

**Why it happens:**
Happy-path connection code; panic-on-error; combined with raw mode this produces the worst UX (Pitfall 11).

**How to avoid:**
- Probe the daemon (`ping`/`version`) before entering the TUI. If it fails, print a plain-text actionable error ("Docker daemon not reachable at /var/run/docker.sock — is it running? Are you in the `docker` group?") to stderr and exit cleanly, NOT inside raw mode.
- Distinguish "daemon down" vs "permission denied" vs "socket missing" and message accordingly.
- If the daemon dies mid-session, show an in-scene banner and keep the last scene rather than crashing.

**Warning signs:**
Tool panics on machines without Docker / without group membership; user sees garbled terminal instead of a clear message.

**Phase to address:** Phase 2 (connection layer), with the panic/raw-mode safety from Phase 1 as backstop.

---

### Pitfall 10: Scene doesn't scale to 50+ containers (layout + perf)

**What goes wrong:**
The rack layout is hand-tuned for ~5–10 containers; at 50+ boxes overlap, racks collide, labels swarm, and per-frame work (sort + project + rasterize every entity, plus 50 concurrent stats streams) pushes Pitfall 3 over the edge. Tiny containers become invisible; one huge container dominates the frame (Pitfall 12).

**Why it happens:**
Developing against a handful of local containers; never tested at fleet scale.

**How to avoid:**
- Define a layout that *grows* (multiple racks/rows, auto-arranged by count) and decide an explicit cap/aggregation strategy beyond N (e.g. group stopped containers, or LOD: distant racks drawn as blocks).
- Budget perf at 50+: profile rasterize time per frame; ensure 50 stats streams (≈50 msgs/sec total) don't dominate. Throttle/aggregate if needed.
- Test with a synthetic generator that spins up 50–100 dummy containers.

**Warning signs:**
Layout looks great with 6 containers, breaks at 30; FPS/CPU degrades linearly with container count; can't find a specific container in the swarm.

**Phase to address:** Phase 3 (scene/layout) for arrangement; verified against Phase 2 data and Phase 1 perf budget.

---

### Pitfall 11: Terminal left broken on panic (raw mode not restored)

**What goes wrong:**
Any panic (a `.unwrap()`, an arithmetic overflow, a NaN box size) while in raw mode + alternate screen leaves the user's terminal with no echo, no line wrapping, cursor hidden, garbled — they must blindly type `reset`. Catastrophic first impression.

**Why it happens:**
Forgetting to install a panic hook; or restoring the terminal only in the normal exit path.

**How to avoid:**
- Install a panic hook at startup that restores the terminal (`disable_raw_mode`, `LeaveAlternateScreen`, show cursor) *before* printing the panic. ratatui provides this (`ratatui::init` sets a panic hook; or save the old hook with `std::panic::take_hook()` and restore terminal then call it). Note: there are known backend-specific bugs (ratatui issue #1005, termion) — verify restoration actually works by deliberately panicking in a test.
- Also restore on Ctrl-C/SIGTERM, not just clean quit.
- Eliminate panic sources in the hot path: no `unwrap` on Docker/stats, clamp NaN/Inf, use checked math for sizing.

**Warning signs:**
After a crash the terminal is unusable until `reset`; cursor missing; typed input invisible.

**Phase to address:** Phase 1 (terminal lifecycle) — install the hook in the very first runnable version.

---

### Pitfall 12: Resource→size mapping makes small containers invisible / large ones dominate

**What goes wrong:**
Mapping CPU/RAM linearly to box size means an idle container (0.1% CPU) is a sub-pixel dot while a busy one fills the screen; the scene can't show both. Or everything maps to nearly the same size and the "sized by usage" signal is lost.

**Why it happens:**
Linear mapping of an unbounded, heavy-tailed quantity (CPU%/RAM ranges over orders of magnitude) onto a bounded visual dimension.

**How to avoid:**
- Use a **clamped, compressive mapping**: enforce a minimum box size (always legible) and a maximum (never dominates), with a sqrt/log curve in between so the dynamic range fits. Map RAM as a fraction of its limit, not absolute bytes.
- Make "breathing" a *relative* pulse around the box's size, not absolute, so idle boxes still visibly breathe.

**Warning signs:**
Idle containers invisible; one container swallows the frame; all boxes look identical so size conveys nothing.

**Phase to address:** Phase 3 (visual mapping / aesthetics).

---

### Pitfall 13: Camera that induces motion sickness or hides information

**What goes wrong:**
Autopilot that orbits too fast, swings wildly, or constantly changes direction is nauseating; a fixed bad angle hides containers behind others. Manual camera with no constraints lets users get lost (inside a box, below the floor, infinite zoom).

**Why it happens:**
"More motion = more impressive" instinct; unconstrained free camera.

**How to avoid:**
- Autopilot: slow, smooth, eased orbit with a gentle vertical bob; bounded radius; occasional slow re-framing. Easing curves, not linear snaps. PROJECT note: "mesmerizing for 10s then boring" → vary the framing slowly (drift focus between racks) to sustain interest without frantic motion.
- Manual: constrain pitch (avoid gimbal flip), clamp zoom min/max, keep the floor in view, smooth (lerp) camera transitions. Any input → switch to manual (already specced).

**Warning signs:**
Testers feel queasy; "I can't tell what I'm looking at"; camera clips inside geometry; gets boring within seconds or is exhausting to watch.

**Phase to address:** Phase 3 (camera/UX).

---

### Pitfall 14: Resize and SSH/latency not handled → corruption and lag

**What goes wrong:**
On terminal resize, the braille framebuffer keeps old dimensions → garbage, panics on out-of-bounds, or letterboxed junk. Over SSH, per-frame full-screen truecolor writes are large; high latency/low bandwidth makes the app lag, tear, or flicker (Pitfall: write throughput).

**Why it happens:**
Buffer sized once at startup; assuming local terminal bandwidth.

**How to avoid:**
- Handle the resize event (crossterm `Event::Resize`): reallocate the braille buffer and recompute projection viewport. Clamp all draw operations to current bounds.
- For SSH: rely on ratatui's built-in cell diffing (it already writes only changed cells), keep frame rate modest, and offer a "low bandwidth" mode (lower FPS, fewer colors, simpler shading). A full-screen braille frame changes nearly every cell, so diffing helps little under motion — the real levers are FPS cap and color depth.

**Warning signs:**
Garbled output / panic after resizing the window; severe lag or tearing over SSH; high bytes/sec on the wire.

**Phase to address:** Phase 1 (resize + buffer management); SSH/low-bandwidth mode validated in Phase 3.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Hardcode aspect-correction = 2.0 in projection math | Fast first render | Wrong on many fonts; scattered magic number to fix everywhere | Only as a *named constant* in one place; never inline |
| `.unwrap()` on Docker/stats results | Fast happy path | Panics in raw mode → broken terminal (Pitfall 11) | Never in the hot path; only in startup probe that has a panic hook |
| Enumerate containers once at startup | Simplicity | Misses new/removed containers (Pitfall 7) | MVP demo only; must add events reconciliation before "live" claim |
| Per-pixel z-buffer at braille resolution | "Correct" occlusion | Expensive, feeds Pitfall 3 | Only if painter's algorithm visibly fails |
| Author themes in truecolor only | Looks great locally | Breaks over SSH/tmux (Pitfall 6) | Never without a quantization/fallback path |
| Re-poll stats per render frame | Simple data flow | Hammers daemon, couples render to I/O | Never — decouple cadences |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Docker stats API | Display `total_usage` directly / no delta | Compute `(cpu_delta/system_delta)*online_cpus*100`; discard first sample; guard div-by-zero |
| Docker stats (memory) | Show `memory_stats.usage` as used RAM | Subtract cache/`inactive_file`; express vs limit |
| bollard streaming | Manually delta across your own polls | Use `stream:true`; precpu is filled per stream sample by Docker |
| Container lifecycle | Static enumeration | Subscribe to `events()`; reconcile entity set; cancel per-container tasks on death |
| Docker socket | `.unwrap()` connect | Probe before TUI; clean stderr message for down/permission/missing |
| online_cpus / system_cpu_usage | Assume present & nonzero | Treat as Option; fallback to `percpu_usage.len()`; clamp |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Busy render loop | Core pegged at 100%, fan spins on launch | Frame tick via `tokio::select!`/interval; render-on-change | Immediately, even with 0 containers |
| Re-rasterize identical static scene | High idle CPU in manual mode | Skip redraw when camera+stats unchanged | Idle / manual mode |
| 50+ concurrent stats streams + per-frame project/sort | FPS drops, CPU climbs with container count | Decouple cadences; LOD/aggregate distant racks; profile at 50+ | ~30–50 containers |
| Full-screen truecolor writes over SSH | Lag, tearing, high bytes/sec | FPS cap + low-bandwidth/color mode; rely on cell diff | High-latency/low-bw links |
| Blocking rasterize on tokio worker | Stats freeze during camera motion | Render off the async workers; channels for data | Under load / many containers |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Any write/mutate call to Docker API | Violates read-only invariant; could stop/remove containers | Use only read endpoints (list/inspect/stats/events); no start/stop/exec/rm in the codebase at all |
| Logging container env/secrets in detail panel | Leaks secrets to screen/logs over SSH/screenshare | Don't surface env vars/secrets; if shown, redact; never log them |
| Trusting daemon to enforce read-only | Future code could regress to mutation | Keep mutating bollard calls out of the dependency surface; review |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Frantic/fast autopilot orbit | Motion sickness, can't read | Slow eased orbit, bounded radius, gentle bob |
| Autopilot boring after 10s | User closes it | Slowly drift focus between racks; subtle variation, not frantic motion |
| Linear size mapping | Idle containers invisible / busy one dominates | Clamped log/sqrt mapping, min & max box size |
| Labels everywhere | Unreadable swarm | Label focused/nearest only; full names in 2D panel |
| Truecolor-only theme | Unreadable over SSH/16-color/colorblind | Detect depth, quantize, distinguish status by brightness+shape, high-contrast fallback |
| No depth cues | Flat ambiguous overlap | Painter's sort + fog/dimming + solid faces |

## "Looks Done But Isn't" Checklist

- [ ] **Single cube render:** Looks like a *cube* (not a brick) and reads as 3D — verify aspect correction with a wireframe sphere (round, not egg).
- [ ] **CPU%:** Matches `docker stats` CLI within a few % — verify against the CLI for a busy and an idle container; first frame not garbage.
- [ ] **Idle CPU:** Autopilot idle sits at single-digit % of one core — verify with `top` for 60s.
- [ ] **Panic safety:** Deliberately panic → terminal fully restored (echo, cursor, no raw mode) — verify, including the known ratatui backend bug.
- [ ] **Daemon down:** Run with Docker stopped / outside docker group → clean stderr message, not a garbled crash.
- [ ] **Lifecycle:** Start & stop containers while running → boxes appear/disappear gracefully, no panic, no orphan tasks.
- [ ] **Scale:** Run with 50+ synthetic containers → layout readable, FPS/CPU within budget, labels not a swarm.
- [ ] **Resize:** Shrink/grow the window → no garbage, no panic.
- [ ] **SSH:** Run over a real SSH session → usable, not laggy; colors acceptable.
- [ ] **Themes:** Each theme readable on 256-color and 16-color terminals; statuses distinguishable beyond hue.

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Aspect ratio wrong (Pitfall 2) | LOW if isolated | Adjust the single `cell_aspect` constant; HIGH if magic numbers spread — refactor to one knob |
| CPU% formula wrong (Pitfall 1) | LOW | Fix normalizer + add synthetic-sample unit tests; localized to data layer |
| Busy loop / CPU peg (Pitfall 3) | MEDIUM | Introduce frame tick + render-on-change; may ripple into loop architecture |
| Terminal broken on panic (Pitfall 11) | LOW | Add panic hook + restore on signals; remove hot-path unwraps |
| Scene unreadable at scale (Pitfall 10) | HIGH | Rework layout (LOD/multi-rack) + labeling + size mapping together |
| Truecolor-only themes (Pitfall 6) | MEDIUM | Add depth detection + quantization; ensure no inline RGB at draw sites |

## Pitfall-to-Phase Mapping

Assumes a lean 4-phase roadmap:
**P1 Render core** (loop, terminal lifecycle, rasterizer, camera math) ·
**P2 Docker data** (connect, stats normalize, events reconcile, streaming tasks) ·
**P3 Scene & aesthetics** (layout, depth cues, labels, themes, size mapping, camera feel, SSH mode) ·
**P4 Polish/validate** (scale soak, theme matrix, panic/resize hardening).

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| 1. CPU% delta gotcha | P2 | CPU% matches `docker stats` CLI; no NaN; first sample handled (unit tests) |
| 2. Cell aspect ratio | P1 | Wireframe sphere renders round, cube renders cubic |
| 3. Render-loop CPU peg | P1 | Idle autopilot single-digit % of one core for 60s (`top`) |
| 4. Depth/occlusion | P1 (sort/fog) → P3 (refine) | No far-through-near bleed; depth reads clearly |
| 5. Moving labels | P3 | Focused label readable & stable during orbit; no swarm |
| 6. Color depth/contrast | P3 (palette) / P1 (no inline RGB) | Each theme readable on 256 & 16 color; status distinguishable beyond hue |
| 7. Container lifecycle | P2 | Start/stop containers live → graceful add/remove, no panic/leak |
| 8. Blocking async runtime | P1/P2 | Stats keep flowing during camera motion (no freeze) |
| 9. Daemon down/permission | P2 (+P1 backstop) | Clean stderr message with Docker stopped / non-group user |
| 10. 50+ container scale | P3 (layout) / P4 (soak) | 50+ synthetic containers readable & within perf budget |
| 11. Panic → broken terminal | P1 | Deliberate panic restores terminal fully |
| 12. Size mapping | P3 | Idle visible, busy bounded, size conveys signal |
| 13. Camera sickness/boredom | P3 | Testers comfortable; sustained interest; no geometry clipping |
| 14. Resize / SSH | P1 (resize) / P3 (SSH mode) | Resize no garbage; SSH usable & not laggy |

## Sources

- Docker stats CPU% delta formula — [moby/moby issue #29306](https://github.com/moby/moby/issues/29306), [Docker forums: Calculate CPU usage in percentage with latest API](https://forums.docker.com/t/calculate-cpu-usage-in-percentage-with-latest-api/66769), [gist: How To Get Docker Container CPU Utilization](https://gist.github.com/BeardedDonut/5e3643b06e90ce4d41197d8cfb8fb0b5) (HIGH)
- bollard `Stats` / `StatsOptions` (stream, one_shot, precpu_stats) — [docs.rs bollard::container::Stats](https://docs.rs/bollard/latest/bollard/container/struct.Stats.html), [fussybeaver/bollard](https://github.com/fussybeaver/bollard) (HIGH)
- Braille 2×4 cell grid & aspect-ratio distortion — [termdot (U+2800–U+28FF, 2×4 grid)](https://github.com/ahmadawais/termdot), [drawille](https://github.com/asciimoo/drawille), [Terminal graphics with Braille characters](https://danieledapo.github.io/post/terminal-graphics-braille/), [ytop issue #79: braille graphs misbehave](https://github.com/cjbassi/ytop/issues/79) (HIGH)
- ratatui panic hooks / terminal restore — [Ratatui: Setup Panic Hooks](https://ratatui.rs/recipes/apps/panic-hooks/), [ratatui issue #1005 (termion restore bug)](https://github.com/ratatui/ratatui/issues/1005) (HIGH)
- ratatui buffer diffing / double-buffer (only changed cells written) — [Ratatui: Rendering under the hood](https://ratatui.rs/concepts/rendering/under-the-hood/) (HIGH)
- Domain experience: terminal-3D legibility, camera UX, painter's algorithm, resource-to-size mapping (MEDIUM — established graphics/UX practice, not a single citation)

---
*Pitfalls research for: terminal software-3D + real-time Docker stats (Rust TUI)*
*Researched: 2026-05-26*
