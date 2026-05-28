# Requirements: 3dd (3D Docker)

**Defined:** 2026-05-26
**Core Value:** A beautiful, legible 3D scene that lets you grasp the state of your Docker environment at a glance — what's alive, what's hot, what's connected to what.

## v1 Requirements

Full entity set bundled per PROJECT.md, but build order sequences render-correctness before entity richness (see roadmap).

### Render Core

- [x] **REND-01**: Scene renders as 3D in the terminal via a braille/ASCII rasterizer (projection + rasterize pipeline)
- [x] **REND-02**: Faces/boxes are painter's-sorted back-to-front so nearer objects occlude farther ones (reads as 3D, not flat noise)
- [x] **REND-03**: Boxes are depth-shaded (per-face brightness + distance dimming/fog) so they read as solid forms
- [x] **REND-04**: Aspect-ratio correction makes a cube look cubic, via a single config-exposed correction factor (not scattered constants)
- [x] **REND-05**: Terminal restores cleanly on exit and on panic (raw mode never left enabled)
- [x] **REND-06**: Scene re-layouts correctly on terminal resize
- [x] **REND-07**: Render loop stays smooth at a steady tick without pegging a CPU core (FPS cap / render-on-change, no spin loop)

### Docker Data

- [x] **DOCK-01**: App lists containers and inspects them from the local Docker daemon (bollard)
- [x] **DOCK-02**: Live stats stream feeds CPU% and memory, with the correct cumulative-counter delta formula and guards against NaN/empty first sample
- [x] **DOCK-03**: Scene rebuilds as containers are created/destroyed (Docker events drive add/remove)
- [x] **DOCK-04**: Stat update rate is decoupled from render rate (slow Docker data still yields smooth motion)

### Container Visuals

- [x] **CONT-01**: Each container is a box colored by status (running / paused / stopped / restarting / crashed)
- [x] **CONT-02**: Box dimensions scale with CPU/RAM usage, clamped/normalized so idle isn't invisible and a hog doesn't fill the screen
- [ ] **CONT-03**: Boxes "breathe" — current size/state interpolates toward live-stat targets
- [ ] **CONT-04**: Each container shows a name label, billboarded and occlusion-aware (handles overlap)
- [x] **CONT-05**: Containers occupy a stable rack/datacenter slot that persists frame-to-frame (no jitter/teleport)

### Camera & Interaction

- [~] **CAM-01**: Autopilot orbit camera runs by default (slow continuous orbit, screensaver feel, motion parallax) — Phase 2 shipped per-box self-spin + static scene-framed camera instead of whole-scene orbit (human override, approved live). RECONCILE: motion model changed; revisit orbit vs per-box-spin in Phase 4 (CAM-02/03 manual explore).
- [ ] **CAM-02**: Manual explore mode allows WASD/arrow orbit and zoom around the scene
- [ ] **CAM-03**: Any user input switches from autopilot to manual explore
- [ ] **CAM-04**: User can Tab-cycle through containers; the selected box is highlighted (outline/pulse)
- [ ] **CAM-05**: Enter on the selected container opens a 2D detail panel surfacing health, restart count, uptime, block I/O, ports, and mounts

### Entities

- [ ] **ENT-01**: Networks are visualized as floor-planes; containers are placed on the plane of their network (layout is network-aware)
- [ ] **ENT-02**: Published ports render as glowing emissive points on the box face
- [ ] **ENT-03**: Volumes render as cylinders/disks attached to their container; sized by a proxy (NOT real bytes — Docker API has no cheap size source)
- [ ] **ENT-04**: Images render as layered stacks in a separate scene region, layer count/size from image data

### Theming & Robustness

- [x] **THEME-01**: Color comes from a palette abstraction (status colors + bg + glow); no status color is hardcoded
- [ ] **THEME-02**: Three built-in palette presets ship: cyberpunk neon, terminal-green, Notion-soft
- [ ] **THEME-03**: A palette is derived from the current Omarchy/config theme (parsed from ~/.config)
- [ ] **THEME-04**: Palettes are switchable at runtime (hot re-map, no scene rebuild)
- [ ] **THEME-05**: A persistent legend HUD shows the active status color key
- [ ] **THEME-06**: Config and themes load from a TOML file (serde)
- [x] **ROB-01**: Graceful states for daemon-down, zero-containers, and permission errors (never a crash or blank void)
- [ ] **ROB-02**: SSH/degrade path auto-detects terminal capability and downshifts braille→ASCII with an FPS cap

## v2 Requirements

Deferred to future release. Tracked but not in current roadmap.

### Visual Emphasis

- **EMPH-01**: Hot-emphasis pulse/glow modulated by live CPU (push "what's hot" past size alone)
- **EMPH-02**: Health badge/ring on the box when a healthcheck is defined (status ≠ health)

### Grouping

- **GRP-01**: Read-only compose grouping — visually cluster containers by `com.docker.compose.project` label

### On-Demand Data

- **DATA-01**: On-demand volume size scan (`docker system df -v`) behind an explicit key
- **DATA-02**: Network wires (in addition to / instead of floor-planes) for comparison
- **DATA-03**: dive-style wasted-space/efficiency score on images

## Out of Scope

Explicitly excluded. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| Container control (start/stop/restart/exec/rm) | Violates read-only identity; defer to lazydocker for ops |
| Compose orchestration (up/down) | Mutation; only read-only grouping is ever allowed |
| Remote / multi-host / swarm | Connection/auth/latency complexity; local daemon only for MVP |
| In-scene log streaming | Text-heavy, fights the 3D aesthetic, not "at a glance" |
| Historical graphs / time-series | 3dd is a live observatory, not a metrics DB; breathing IS the time dimension |
| Native GPU window / web frontend | TUI-only is the core identity & the SSH advantage |
| Mouse-required 3D manipulation | Unreliable over SSH/tmux; keyboard-first matches TUI ethos |
| Photorealistic models / textures | Terminal resolution caps detail; lean into stylized boxes + neon |
| Alerting / notifications on crash | Scope creep into ops/paging; the red box IS the passive alert |
| Real-time auto volume sizing | `system df -v` scans disk, breaks smooth-render constraint (see DATA-01) |

## Traceability

Which phases cover which requirements. Updated by create-roadmap.

| Requirement | Phase | Status |
|-------------|-------|--------|
| REND-01 | Phase 1 | Complete |
| REND-02 | Phase 1 | Complete |
| REND-03 | Phase 1 | Complete |
| REND-04 | Phase 1 | Complete |
| REND-05 | Phase 1 | Complete |
| REND-06 | Phase 1 | Complete |
| REND-07 | Phase 1 | Complete |
| DOCK-01 | Phase 3 | Complete |
| DOCK-02 | Phase 3 | Complete |
| DOCK-03 | Phase 3 | Complete |
| DOCK-04 | Phase 3 | Complete |
| CONT-01 | Phase 2 | Complete |
| CONT-02 | Phase 2 | Complete |
| CONT-03 | Phase 4 | Pending |
| CONT-04 | Phase 4 | Pending |
| CONT-05 | Phase 2 | Complete |
| CAM-01 | Phase 2 | Complete (deviation — see CAM-01 note) |
| CAM-02 | Phase 4 | Pending |
| CAM-03 | Phase 4 | Pending |
| CAM-04 | Phase 4 | Pending |
| CAM-05 | Phase 4 | Pending |
| ENT-01 | Phase 4 | Pending |
| ENT-02 | Phase 4 | Pending |
| ENT-03 | Phase 4 | Pending |
| ENT-04 | Phase 4 | Pending |
| THEME-01 | Phase 1 | Complete |
| THEME-02 | Phase 5 | Pending |
| THEME-03 | Phase 5 | Pending |
| THEME-04 | Phase 5 | Pending |
| THEME-05 | Phase 5 | Pending |
| THEME-06 | Phase 5 | Pending |
| ROB-01 | Phase 3 | Complete |
| ROB-02 | Phase 5 | Pending |

**Coverage:**
- v1 requirements: 33 total
- Mapped to phases: 33
- Unmapped: 0 ✓

---
*Requirements defined: 2026-05-26*
*Last updated: 2026-05-26 after initial definition*
