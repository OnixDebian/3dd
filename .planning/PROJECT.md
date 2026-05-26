# 3dd (3D Docker)

## What This Is

A terminal-based 3D visualizer for Docker, rendered entirely in the TUI in the
aesthetic of [ratty](https://github.com/orhun/ratty). It shows your local Docker
state as a living 3D "server room": containers are blade-servers in racks,
colored by status and sized by resource usage, with live-streaming stats. It is
a visual artifact / observatory — not a daily ops replacement for lazydocker.

## Core Value

A beautiful, legible 3D scene that lets you grasp the state of your Docker
environment at a glance — which containers are alive, which are hot, what's
connected to what. If everything else fails, the scene must look good and read
clearly.

## Requirements

### Validated

(None yet — ship to validate)

### Active

- [ ] Render a 3D scene in the terminal (braille/ASCII rasterizer, ratty-style)
- [ ] Datacenter metaphor: containers as blade-servers arranged in racks
- [ ] Containers colored by status (running / paused / stopped / restarting / crashed)
- [ ] Container box dimensions scale with resource usage (CPU / RAM)
- [ ] Live-streaming stats from Docker API (~realtime, boxes "breathe")
- [ ] Networks visualized (floor/plane per network or wires between containers)
- [ ] Volumes visualized (cylinders/disks attached to containers)
- [ ] Images visualized as layered stacks; ports as glowing points
- [ ] Autopilot camera mode (orbits the scene hands-free — screensaver feel)
- [ ] Manual explore mode (WASD/arrows orbit, Tab cycles containers, Enter shows detail panel)
- [ ] Default to autopilot; any input switches to manual explore
- [ ] Themeable color palettes via config (cyberpunk neon, terminal-green, Notion-soft), switchable at runtime

### Out of Scope

- Container control actions (start/stop/restart/exec/remove) — this is a visualizer, not a manager; defer to lazydocker for ops
- Docker Compose orchestration — only read/display, never mutate
- Remote/multi-host Docker — local daemon only for MVP
- Native GPU window / web frontend — TUI-only by design (the ratty aesthetic is the point)
- Distribution polish (README gifs, binary releases, packaging) — deferred until after personal MVP

## Context

- Reference aesthetic: ratty (https://github.com/orhun/ratty) — 3D rendered in the
  terminal via braille/ASCII, neon glow.
- Reference tool for feature surface: lazydocker (containers, stats, logs, images,
  volumes, networks, compose grouping).
- Likely stack: Rust + ratatui (TUI) + custom software 3D projector/rasterizer +
  bollard (Docker API client, supports streaming stats/events/logs).
- Target environment: Linux (Arch/Omarchy, Wayland), runs in a normal terminal;
  should also be usable over SSH.
- Audience: personal first, open-source later — keep code clean enough to publish
  but don't pay distribution costs yet.

## Constraints

- **Tech stack**: TUI-only (no GPU window, no browser) — the terminal-3D aesthetic is the core identity, not an implementation detail
- **Platform**: Local Docker daemon on Linux for MVP — no remote/multi-host
- **Performance**: Live stats streaming must stay smooth in a terminal; 3D rasterizer and stats polling must not peg CPU
- **Read-only**: Never mutate Docker state — pure observability

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| TUI rendering (Rust + ratatui), not native GPU/web | Ratty-style terminal 3D is the whole point; runs over SSH; hacker aesthetic | — Pending |
| Datacenter/rack metaphor for layout | Most legible mapping of "containers as servers"; user picked it over k8s-city/space/force-graph | — Pending |
| Read-only visualizer, not an ops tool | Goal is aesthetics + at-a-glance comprehension, not replacing lazydocker | — Pending |
| Live-stream stats (vs poll/snapshot) | "Breathing" boxes — the living quality is core to the appeal | — Pending |
| Both camera modes, autopilot default | Works as a passive screensaver and as an explorable scene | — Pending |
| Themeable palettes via config | User wants flexibility over a single fixed look | — Pending |
| Bundle full entity set in MVP (containers/stats/networks/volumes/images/ports) | User wants the complete picture, not an incremental rollout | — Pending |

---
*Last updated: 2026-05-26 after initialization*
