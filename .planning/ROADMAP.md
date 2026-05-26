# Roadmap: 3dd (3D Docker)

## Overview

A render-first journey: prove that terminal-3D actually reads as 3D before any
Docker plumbing exists, then generalize one cube into a full synthetic scene,
feed real Docker data into the proven renderer, make it live and explorable with
the full entity set, and finish with theming, config, and cross-terminal
robustness. The novel risk (3D legibility in braille) is front-loaded; the
well-understood Docker work comes only once the renderer is trustworthy.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

- [ ] **Phase 1: Render Core & Legibility Spike** - App skeleton + one aspect-correct, depth-shaded cube that orbits smoothly at low CPU
- [ ] **Phase 2: Scene Pipeline & Layout** - Many synthetic boxes in a stable rack layout, occluded, status-colored, load-sized, autopilot orbit
- [ ] **Phase 3: Docker Data Layer** - Real containers + correct live stats + events feed the proven renderer
- [ ] **Phase 4: Animation, Interaction & Full Entity Set** - Breathing boxes, manual explore + detail panel, networks/volumes/images/ports
- [ ] **Phase 5: Theming, Config & Robustness** - TOML config, runtime palette switching, legend HUD, SSH/degrade path

## Phase Details

### Phase 1: Render Core & Legibility Spike
**Goal**: Prove terminal-3D reads as 3D — a single aspect-correct, depth-shaded cube orbits smoothly without pegging a CPU core, on a panic-safe app skeleton.
**Depends on**: Nothing (first phase)
**Requirements**: REND-01, REND-02, REND-03, REND-04, REND-05, REND-06, REND-07, THEME-01
**Success Criteria** (what must be TRUE):
  1. A cube renders in the terminal and visibly reads as a solid 3D form (faces occlude back-to-front, depth-shaded, fog/distance dimming)
  2. The cube looks cubic, not squashed — aspect correction is a single config-exposed factor
  3. The cube orbits smoothly at a steady tick without pegging a CPU core (FPS cap / render-on-change)
  4. Quitting or panicking always restores the terminal cleanly; resizing re-layouts the scene
  5. All color comes from a palette abstraction — no status/glow color is hardcoded
**Research**: Likely (novel — the project's #1 risk)
**Research topics**: framebuffer→braille coordinate mapping (Canvas math-coords bottom-left vs 2×4 grid top-left); empirical aspect-correction constant (font/terminal-dependent, ship as knob); painter's-sort vs z-buffer decision
**Plans**: TBD

Plans:
- [ ] 01-01: TBD

### Phase 2: Scene Pipeline & Layout
**Goal**: Generalize the one cube into a full scene of synthetic entities — many boxes in a stable rack/datacenter layout, painter's-occluded, status-colored and load-sized, under an autopilot orbit camera.
**Depends on**: Phase 1
**Requirements**: CONT-01, CONT-02, CONT-05, CAM-01
**Success Criteria** (what must be TRUE):
  1. Dozens of synthetic boxes render in a rack/datacenter layout and the scene still reads clearly (no noise collapse at scale)
  2. Each box is colored by status and sized by a (synthetic) CPU/RAM proxy, clamped so idle isn't invisible and a hog doesn't fill the screen
  3. Boxes hold a stable layout slot frame-to-frame (no jitter/teleport)
  4. The autopilot orbit camera runs by default with smooth motion parallax
**Research**: Likely (load-bearing, unresolved)
**Research topics**: rack-grid vs network-grouped-floor layout algorithm — design network-aware from the start even though networks land in Phase 4
**Plans**: TBD

Plans:
- [ ] 02-01: TBD

### Phase 3: Docker Data Layer
**Goal**: Feed real Docker data into the proven renderer — list/inspect containers, stream correct live stats, and react to create/destroy events, with graceful failure states.
**Depends on**: Phase 2
**Requirements**: DOCK-01, DOCK-02, DOCK-03, DOCK-04, ROB-01
**Success Criteria** (what must be TRUE):
  1. The scene shows the user's actual local containers (listed + inspected via bollard)
  2. CPU% and memory are correct (cumulative-counter delta formula, guarded against NaN/empty first sample)
  3. Creating or destroying a container adds/removes its box without restarting the app (events drive the scene)
  4. Stat update rate is decoupled from render rate — slow Docker data still yields smooth motion
  5. Daemon-down, zero-containers, and permission errors show a graceful state, never a crash or blank void
**Research**: Unlikely (bollard well-documented; CPU% delta gotcha already captured in research PITFALLS)
**Plans**: TBD

Plans:
- [ ] 03-01: TBD

### Phase 4: Animation, Interaction & Full Entity Set
**Goal**: Make the scene live and explorable — breathing boxes, manual orbit/select/detail interaction, and the remaining entities (networks, ports, volumes, images).
**Depends on**: Phase 3
**Requirements**: CONT-03, CONT-04, CAM-02, CAM-03, CAM-04, CAM-05, ENT-01, ENT-02, ENT-03, ENT-04
**Success Criteria** (what must be TRUE):
  1. Boxes "breathe" — size/state eases toward live-stat targets instead of snapping
  2. Any input drops out of autopilot into manual explore (WASD/arrow orbit + zoom)
  3. Tab cycles containers with the selection highlighted; Enter opens a 2D detail panel (health, restart count, uptime, block I/O, ports, mounts)
  4. Each container shows a billboarded, occlusion-aware name label that handles overlap
  5. Networks render as floor-planes with containers placed on their network's plane; ports glow on box faces; volumes are proxy-sized disks; images are layered stacks
**Research**: Likely (hardest text problem)
**Research topics**: label occlusion in braille (overlapping moving billboards) — prototype early; volume-size proxy metric (Docker API has no cheap size source)
**Plans**: TBD

Plans:
- [ ] 04-01: TBD

### Phase 5: Theming, Config & Robustness
**Goal**: Deliver the themeable-palette requirement and cross-terminal robustness — TOML config, runtime palette switching, legend HUD, and an SSH/degrade path.
**Depends on**: Phase 4
**Requirements**: THEME-02, THEME-03, THEME-04, THEME-05, THEME-06, ROB-02
**Success Criteria** (what must be TRUE):
  1. Three palette presets ship (cyberpunk neon, terminal-green, Notion-soft) and a palette can be derived from the Omarchy/config theme in ~/.config
  2. Palettes switch at runtime as a hot re-map (no scene rebuild)
  3. A persistent legend HUD shows the active status color key
  4. Config and themes load from a TOML file via serde
  5. Over SSH / weak terminals the app auto-detects capability and downshifts braille→ASCII with an FPS cap
**Research**: Unlikely (serde/toml + theming are standard patterns)
**Plans**: TBD

Plans:
- [ ] 05-01: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Render Core & Legibility Spike | 0/TBD | Not started | - |
| 2. Scene Pipeline & Layout | 0/TBD | Not started | - |
| 3. Docker Data Layer | 0/TBD | Not started | - |
| 4. Animation, Interaction & Full Entity Set | 0/TBD | Not started | - |
| 5. Theming, Config & Robustness | 0/TBD | Not started | - |
