# Feature Research

**Domain:** Terminal-based 3D Docker visualizer (read-only TUI observatory)
**Researched:** 2026-05-26
**Confidence:** HIGH (Docker data surface, tool feature convergence) / MEDIUM (3D-in-terminal legibility heuristics — grounded in depth-perception literature + ratty/braille demos, but 3dd's exact metaphor is novel and unvalidated)

## Context From Reference Tools

Studied: lazydocker, ctop, oxker, dive, `docker stats`. Convergence across all four
container tools defines what "any Docker viewer" surfaces. dive is the outlier — it
targets *images* (layers, wasted space), not live containers.

| Tool | Surfaces | Live? | Relevance to 3dd |
|------|----------|-------|------------------|
| `docker stats` | name, CPU%, mem usage+limit+%, net I/O, block I/O, PIDs | yes (stream) | The canonical metric set — bollard exposes the same stream |
| ctop | + status, sortable columns, single-container inspect, block I/O | yes | Confirms block I/O is part of the expected set |
| oxker (Rust) | container list, status, CPU, mem, **ports**, logs, search | yes | Closest stack analog; proves ports belong in the at-a-glance view |
| lazydocker | + logs graphs, env/config inspect, compose grouping, images/volumes/networks panes | yes | Defines the full *entity* surface (not just containers) |
| dive | image layers, layer sizes, wasted space, efficiency score | no (snapshot) | Informs the "images as layered stacks" visual; layer sizes are real |

**Key Docker-data realities (HIGH confidence, affects feature feasibility):**
- Status, CPU%, mem, net I/O, block I/O all stream cheaply via the stats API (bollard `stats`).
- Health (`State.Health.Status`), restart count (`RestartCount`), uptime (`State.StartedAt`),
  exit code, ports come from `inspect` — cheap, but a separate call from the stats stream.
- **Volume size is NOT in the Docker API by default.** Requires `docker system df -v`
  (expensive, scans disk) or mounting + `du`. This is a hard constraint on volume visuals.
- Network membership (which containers on which network, subnet, driver, scope) comes from
  `network inspect` / container inspect — cheap and reliable.

## Feature Landscape

### Table Stakes (Scene is useless/illegible without these)

For 3dd, "table stakes" = the scene fails its core promise ("grasp Docker state at a glance")
without it. Split into **data legibility** and **3D legibility** (the latter from depth-cue research).

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Container as box, colored by status | Status is THE first question; color is the fastest visual channel. All four tools lead with status. | LOW | running/paused/stopped/restarting/crashed → 5-color map. Maps to PROJECT requirement. |
| Box size scales with CPU/RAM | The "what's hot" question. Size is a strong pre-attentive channel; lets you see load without reading numbers. | MED | Need clamp/normalize so idle≠invisible and one hog≠fills screen. Log scale likely. |
| Live stats stream ("breathing") | The living quality is the stated core appeal; static = just a diagram. | MED | bollard stats stream per container; throttle to ~1–2 Hz to avoid CPU peg (perf constraint). |
| Container labels (name) | A blade you can't identify is decoration, not information. Occlusion/depth makes unlabeled boxes ambiguous. | MED | Billboard text near box; must handle overlap/occlusion. Hardest TUI text problem. |
| Occlusion + depth ordering (painter's sort) | Occlusion is the single strongest depth cue (depth-perception lit). Without correct front/back sort the scene reads as flat noise. | MED | Z-sort faces/boxes back-to-front before braille rasterize. Non-negotiable for "reads as 3D." |
| Shading / brightness by depth or normal | Flat-colored boxes have no form. Lambert-ish shading + depth fog is what makes braille demos read as solid. | MED | Cheap per-face dot-product shade; dim distant boxes (atmospheric depth cue). |
| Rack/grid layout (deterministic placement) | Random placement = chaos. A stable datacenter grid is the legibility backbone. | MED | Stable slot assignment so a container keeps its place across frames (no jitter/teleport). |
| Autopilot orbit camera (default) | Stated default mode; provides motion parallax — a real depth cue, not just eye-candy. | MED | Slow continuous orbit. Doubles as the screensaver value prop. |
| Status legend / color key | Themeable palettes mean colors aren't self-evident; viewer must decode what cyan vs magenta means. | LOW | Small persistent HUD corner. Trivial but easy to forget. |
| Empty/connecting/error states | Docker socket down, zero containers, daemon starting — must not crash or show a blank void. | LOW | Graceful "no containers" / "cannot reach daemon" scene. |

### Differentiators (Why look at 3dd vs flat lazydocker)

These are where 3dd competes. They align with Core Value (at-a-glance comprehension + beauty).

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Networks as floor-planes / wires | Spatial grouping answers "what's connected" instantly — flat tools bury this in an inspect pane. This is 3dd's unique read. | HIGH | Floor-plane per network (containers sit on their net) is more legible than crossing wires. Wires risk spaghetti. |
| Ports as glowing points | "What's exposed" at a glance; glow exploits the neon aesthetic. oxker proves ports matter at-a-glance. | MED | Emissive dots on the box face; only published host:container ports. |
| Volumes as cylinders/disks attached | Shows persistence + attachment visually; cylinders read as "storage." | MED | Attachment is reliable; **size is NOT (API gotcha)** — size cylinders by a proxy (fixed/by mount count), not real bytes, unless opt-in df scan. |
| Images as layered stacks | dive's layer concept in 3D; layer count/size is real data. Visually distinct from containers. | MED | Layer sizes available; wasted-space score is a dive-only extra (defer). |
| Themeable palettes, runtime-switchable | Stated requirement; cyberpunk/terminal-green/Notion-soft. A signature of the project's identity. | LOW-MED | Palette = struct of status colors + bg + glow. Hot-swap = re-map, no scene rebuild. |
| Manual explore (WASD orbit + zoom) | Turns screensaver into an inspectable scene; motion parallax on demand. | MED | Orbit/zoom/pan around the rack. "Any input switches from autopilot" per PROJECT. |
| Tab-cycle selection + highlight | Lets you walk containers without a mouse; prerequisite for detail. | MED | Highlight = outline/pulse on selected box. Depends on manual mode + stable layout. |
| Detail panel (Enter on selected) | Surfaces the full table-stakes data set (health, restarts, uptime, block I/O, ports, mounts) the 3D view can't show numerically. | MED | 2D ratatui overlay. THIS is where ctop/oxker-level numbers live. Depends on selection. |
| "Hot" emphasis (pulse/glow on high CPU) | Pushes the "what's hot" answer past size alone — motion draws the eye to the busy box. | MED | Modulate glow/scale by live CPU. Builds on breathing + live stats. |
| SSH-friendly degrade (braille→ASCII, FPS cap) | Stated env (SSH-usable). Auto-detect and downshift render mode = works where GPU tools can't. | MED | ratty itself is GPU/Bevy — 3dd's pure-braille path is the actual SSH advantage. |
| Health-state visual (ring/badge on box) | Health ≠ status (a "running" container can be unhealthy). Few flat tools surface it prominently. | LOW-MED | `State.Health.Status` → small badge/ring color. Only if healthcheck defined. |

### Anti-Features (Deliberately NOT building — tied to Out of Scope)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Container control (start/stop/restart/exec/rm) | lazydocker/ctop/oxker all do it; users will expect it. | Violates read-only core identity; mutating state from a "screensaver" is dangerous; doubles the UX surface. | Explicitly "defer to lazydocker for ops." Read-only is a feature, not a gap. |
| Compose orchestration (up/down) | lazydocker groups by compose service. | Mutation; out of scope. | Read-only compose *grouping* (visual cluster by `com.docker.compose.project` label) is fine and cheap — display only. |
| Remote / multi-host / swarm | "Show my whole fleet." | Connection mgmt, auth, latency, layout explosion. Local-only is the MVP constraint. | Local daemon only. Revisit post-MVP. |
| Live log streaming in-scene | lazydocker/oxker show logs; users expect a log pane. | Logs are text-heavy — fights the 3D aesthetic; high bandwidth; not "at a glance." | Out of MVP. If ever, a small detail-panel tail, not a primary view. |
| Real-time volume size (auto) | "How big is my data?" | `docker system df -v` scans disk — slow, pegs I/O, breaks the smooth-render constraint. API doesn't stream it. | Size cylinders by proxy; optional manual "scan sizes" key that runs df once on demand. |
| Historical graphs / time-series | ctop/lazydocker show CPU graphs over time. | 3dd is a live observatory, not a metrics DB; storage + 2D charts dilute the 3D identity. | Show instantaneous state; the "breathing" IS the time dimension. |
| Native GPU window / web frontend | Smoother, prettier; ratty itself is GPU/Bevy. | TUI-only is the stated core identity & SSH value. A GPU window is a different product. | Pure braille/ASCII TUI. The constraint is the point. |
| Mouse-driven 3D manipulation | "Let me drag to rotate." | Mouse over SSH/tmux is unreliable; keyboard-first matches TUI ethos. | WASD/arrows orbit. Mouse optional nice-to-have, never required. |
| Photorealistic models / textures | "Make it look amazing." | Terminal resolution caps detail (lit: best for wireframes/stylized, not photoreal). Wasted effort. | Stylized solid boxes + neon glow + depth fog. Lean into the aesthetic limits. |
| Alerting / notifications on crash | "Tell me when something dies." | Turns a visualizer into a monitoring/paging tool — scope creep into ops. | The red box IS the alert. Visual, passive, glanceable. |

## Feature Dependencies

```
Render core (braille rasterizer + projection + painter-sort)
    └──required-by──> EVERYTHING visual

Docker poll/stream (bollard: list, inspect, stats stream, events)
    └──required-by──> all data-driven visuals

Stable rack layout (deterministic slot assignment)
    └──required-by──> labels, selection highlight, autopilot framing
    └──prevents──────> jitter (containers must keep their slot frame-to-frame)

Live stats stream
    └──required-by──> breathing/box-size, hot-emphasis glow

Status color map  ──depends-on──> Themeable palette (palette defines the colors)
Status legend HUD ──depends-on──> Status color map

Autopilot camera (default)
    └──provides──────> motion parallax (depth cue)

Manual explore camera
    └──required-by──> Tab-cycle selection
                          └──required-by──> Detail panel (Enter)
    └──"any input"───> switches from autopilot to manual

Networks-as-floors ──depends-on──> layout (containers placed ON their network plane)
Ports-as-glow      ──depends-on──> render core (emissive) + inspect data
Volumes-as-cyl     ──depends-on──> attachment data (NOT size — API gotcha)
Images-as-stacks   ──depends-on──> image list + layer data (separate from container scene)

Detail panel ──surfaces──> health, restart count, uptime, block I/O, ports, mounts
            (the numeric data the 3D view deliberately omits)
```

### Dependency Notes

- **Detail panel requires selection, selection requires manual camera + stable layout.**
  Confirms the consumer's stated chain. Build layout → camera → selection → panel in order.
- **Networks-as-floors requires layout to be network-aware**, not just a flat grid. This
  reshapes the layout algorithm — decide early (floor-plane grouping vs pure rack grid).
- **Themeable palette underpins all color**; build the palette abstraction before hardcoding
  any status color, or you'll retrofit it everywhere.
- **Live stats feeds both breathing AND hot-glow** — one stream, two consumers; design the
  stat-update event to fan out.
- **Volume size has no cheap source** — do not let any feature assume real byte sizes for
  volumes without an explicit, opt-in, on-demand df scan.

## MVP Definition

### Launch With (v1) — the scene that reads correctly

- [ ] Braille/ASCII 3D render core with painter's-sort occlusion + depth shading — without occlusion ordering it isn't 3D
- [ ] bollard data layer: list + inspect + stats stream + events (rebuild on create/destroy)
- [ ] Stable rack layout (deterministic slots, no jitter)
- [ ] Containers as status-colored, CPU/RAM-sized, breathing boxes — the core promise
- [ ] Name labels (occlusion-aware billboarding)
- [ ] Autopilot orbit camera (default) + manual WASD orbit/zoom; any input switches mode
- [ ] Themeable palettes (3 presets) switchable at runtime + status legend HUD
- [ ] Tab-cycle selection + detail panel (Enter) surfacing health/restarts/uptime/block-I/O/ports/mounts
- [ ] Networks visualized (floor-planes — most legible per depth research) + ports as glow
- [ ] Empty / daemon-down graceful states
- [ ] SSH degrade path (braille→ASCII fallback, FPS cap)

PROJECT.md asks to bundle the full entity set in MVP; the above keeps that but sequences
*render correctness* before *entity richness*.

### Add After Validation (v1.x)

- [ ] Volumes as cylinders (attachment only) — add once container scene is solid; trigger: scene reads well, want the storage dimension
- [ ] Images as layered stacks (separate view/region) — trigger: container view validated, want the build dimension
- [ ] Hot-emphasis pulse/glow on high CPU — trigger: breathing works but "hot" isn't punchy enough
- [ ] Compose grouping (visual cluster by compose-project label, read-only) — trigger: users with compose stacks find the flat rack cluttered
- [ ] Health badge/ring — trigger: users conflate "running" with "healthy"

### Future Consideration (v2+)

- [ ] On-demand volume size scan (`system df -v`) behind explicit key — defer: perf risk, niche
- [ ] Network wires (in addition to/instead of floors) — defer: spaghetti risk, needs the floor version to compare against
- [ ] dive-style layer wasted-space score on images — defer: nice but image view must exist first

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Render core (occlusion + shading) | HIGH | HIGH | P1 |
| Docker data layer (stream + inspect + events) | HIGH | MED | P1 |
| Status-colored, sized, breathing boxes | HIGH | MED | P1 |
| Stable rack layout | HIGH | MED | P1 |
| Name labels | HIGH | MED | P1 |
| Autopilot + manual camera | HIGH | MED | P1 |
| Themeable palettes + legend | MED | LOW | P1 |
| Selection + detail panel | HIGH | MED | P1 |
| Networks as floor-planes | HIGH | HIGH | P1 |
| Ports as glow | MED | MED | P1 |
| Empty/error states | MED | LOW | P1 |
| SSH degrade path | MED | MED | P1 |
| Volumes as cylinders | MED | MED | P2 |
| Images as layered stacks | MED | MED | P2 |
| Hot-emphasis glow | MED | MED | P2 |
| Compose grouping (read-only) | MED | LOW | P2 |
| Health badge | MED | LOW-MED | P2 |
| On-demand volume size scan | LOW | MED | P3 |
| Network wires | LOW | HIGH | P3 |
| Image wasted-space score | LOW | MED | P3 |

## Competitor Feature Analysis

| Feature | lazydocker / ctop / oxker | dive | Our Approach (3dd) |
|---------|---------------------------|------|--------------------|
| Container status | text/colored row | n/a | Box COLOR — primary visual channel |
| CPU / RAM | number + ASCII graph | n/a | Box SIZE + breathing animation |
| Net / Block I/O | columns / stats pane | n/a | Detail panel only (too numeric for 3D glance) |
| Health / restart / uptime | inspect pane | n/a | Detail panel (+ optional health badge) |
| Ports | column (oxker) / inspect | n/a | Glowing emissive points on box |
| Networks | inspect pane | n/a | **Floor-planes** — spatial grouping (our edge) |
| Volumes | pane (no size) | n/a | Cylinders, attachment-based (size = API gotcha) |
| Images / layers | pane | layer tree + wasted-space score | Layered stacks (visual), score deferred |
| Logs | live pane | n/a | OUT — fights the aesthetic |
| Control actions | yes (mutate) | n/a | OUT — read-only by identity |
| Camera / motion | none (2D panes) | none | Autopilot orbit + manual explore (our edge) |
| Theming | oxker: color scheme in config | none | Runtime-switchable palettes (our edge) |

**Synthesis:** 3dd inverts the flat-tool model. lazydocker/ctop/oxker put *numbers in rows*;
3dd puts *state in space* — color=status, size=load, position=network, motion=liveness. The
numeric layer those tools live in is demoted to an on-demand detail panel. The competitive
moat is the spatial/aesthetic read (networks-as-floors, breathing boxes, orbit camera,
themeable neon), not data coverage — 3dd intentionally surfaces *less raw data*, *more legibly*.

## Sources

### Primary (HIGH confidence)
- [lazydocker docs — what you can manage](https://lazydocker.com/2025/06/29/what-can-i-manage-with-lazydocker/) — entity surface, compose grouping, stats pane
- [oxker (GitHub)](https://github.com/mrjackwills/oxker) — Rust TUI feature set: status, CPU, mem, ports, logs, config color scheme
- [dive (GitHub)](https://github.com/wagoodman/dive) — image layers, layer sizes, wasted-space/efficiency score
- [Docker system df reference](https://docs.docker.com/reference/cli/docker/system/df/) — volume size only via `-v`, expensive
- [Docker runtime metrics](https://docs.docker.com/engine/containers/runmetrics/) — CPU/mem/net/block I/O stats
- [Ratty (GitHub)](https://github.com/orhun/ratty) + [Ratty intro blog](https://blog.orhun.dev/introducing-ratty/) — aesthetic reference; NOTE: ratty is GPU/Bevy terminal emulator, not braille TUI (3dd's braille path differs)
- [Ratty graphics protocol](https://github.com/orhun/ratty/blob/main/protocols/graphics.md) — inline 3D object model

### Secondary (MEDIUM confidence)
- [ctop metrics guide (KX)](https://kx.cloudingenium.com/en/ctop-container-metrics-monitor-docker-podman-terminal-guide/) — default columns: name, CPU, mem, net I/O, block I/O, status
- [docker inspect health/restart fields (Webalert)](https://web-alert.io/blog/docker-container-monitoring-health-checks-uptime) — `State.Health.Status`, `RestartCount`, `StartedAt`
- [docker network inspect (GeeksforGeeks)](https://www.geeksforgeeks.org/devops/what-is-docker-network-inspect/) — driver, scope, subnet, connected containers; volume mountpoint
- [Terminal graphics with Braille (dapo)](https://danieledapo.github.io/post/terminal-graphics-braille/) — 4x2 braille pixel grid technique
- [Depth perception in computer graphics (Cambridge TR-546)](https://www.cl.cam.ac.uk/techreports/UCAM-CL-TR-546.pdf) — occlusion strongest cue, motion parallax, shading/atmospheric depth
- [Inspecting volume size (Medium)](https://medium.com/homullus/how-to-inspect-volumes-size-in-docker-de1068d57f6b) — no API field; mount + du workaround

### Tertiary (LOW confidence — directional only)
- [ProteinView (GitHub)](https://github.com/001TMF/ProteinView) — braille + depth fog + Lambert shading + SSH-aware render modes in a TUI (precedent that 3dd's render approach is viable)
- [ASCII-renderer (GitHub)](https://github.com/ShakedAp/ASCII-renderer) — ASCII-as-pixels 3D precedent

---
*Feature research for: terminal-based 3D Docker visualizer*
*Researched: 2026-05-26*
