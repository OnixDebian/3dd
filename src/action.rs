//! The intent layer: keyboard events are mapped to [`Action`]s, decoupling key
//! bindings from state mutation (04-03).
//!
//! ## Single dispatch surface
//!
//! [`apply_input_action`] is the SINGLE function both backends (braille `App`
//! and `run_kitty`) call to apply a keyboard intent. It is PURE on
//! `(&mut Camera, &mut Selection, &World, &LiveWorld)`: it never touches
//! `bollard`, `tokio::spawn`, or terminal I/O. The Camera/Selection mutations
//! that happen entirely in-process land directly; anything the dispatch can't
//! itself do (because it lacks the docker handle / runtime) surfaces via the
//! [`Effect`] return shape. The caller interprets the Effect in 3-arm match
//! arms.
//!
//! This is the closure shape 04-06 (detail panel) extends: it adds the
//! `Effect::SpawnInspect` interpretation in `App::update` and
//! `run_kitty`'s loop where the docker handle + tokio handle naturally live —
//! WITHOUT modifying `apply_input_action`'s signature. Keeps the dispatch
//! trivially testable (no runtime to construct) and preserves bollard
//! isolation (action.rs imports no bollard).
//!
//! ## Key map
//!
//! | Keys                                | Action                          |
//! |-------------------------------------|---------------------------------|
//! | Left  / 'a'                         | NudgeYaw(-YAW_STEP)             |
//! | Right / 'd'                         | NudgeYaw(+YAW_STEP)             |
//! | Up    / 'w'                         | NudgePitch(+PITCH_STEP)         |
//! | Down  / 's'                         | NudgePitch(-PITCH_STEP)         |
//! | '+' / '=' / PageUp                  | NudgeZoom(-ZOOM_STEP) (closer)  |
//! | '-' / '_' / PageDown                | NudgeZoom(+ZOOM_STEP) (farther) |
//! | Tab                                 | SelectNext                      |
//! | BackTab (Shift+Tab)                 | SelectPrev                      |
//! | Enter                               | OpenDetail                      |
//! | Esc (when detail_open)              | CloseDetail (close popup)       |
//! | Esc (no popup) / 'q' / Ctrl-C       | Quit                            |
//! | 'p' / 'P'                           | CyclePalette                    |
//!
//! Pitfall D: `'a'` and `Left` (and the other WASD/arrow pairs) coexist so
//! AZERTY/Dvorak users still get layout-independent input via arrows.
//!
//! Pitfall E: `KeyEventKind::Press` AND `KeyEventKind::Repeat` are treated
//! identically; `Release` is ignored. Holding a key produces a smooth glide
//! on terminals that surface Repeat events.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::camera::{self, Camera};
use crate::world::live::LiveWorld;
use crate::world::selection::Selection;
use crate::world::World;

/// A high-level input intent produced from a raw key event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Quit the application.
    Quit,
    /// No actionable intent for this key.
    None,
    /// Orbit yaw by the carried delta (radians; sign chooses direction).
    NudgeYaw(f32),
    /// Orbit pitch by the carried delta (radians).
    NudgePitch(f32),
    /// Zoom radius by the carried delta (world units; negative = closer).
    NudgeZoom(f32),
    /// Cycle selection forward through ids-ascending (Tab).
    SelectNext,
    /// Cycle selection backward through ids-ascending (Shift+Tab / BackTab).
    SelectPrev,
    /// Open the detail panel for the current selection (Enter).
    OpenDetail,
    /// Close the detail panel — OR quit, depending on whether one is open
    /// (Esc with no panel = quit; resolved by [`apply_input_action`]).
    CloseDetail,
    /// Cycle the active palette to the next preset (P key). THEME-04.
    CyclePalette,
}

/// Effects an [`Action`] produces that the dispatch surface cannot itself
/// execute (because it lacks the docker handle / tokio runtime).
///
/// [`apply_input_action`] returns these so 04-06 (detail panel) can plug
/// `SpawnInspect` interpretation into `App` / `run_kitty` without modifying
/// the pure dispatch function or violating bollard isolation.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Nothing to do — the action was fully handled by the dispatch.
    None,
    /// Quit the app — caller sets `should_quit = true` / breaks its loop.
    Quit,
    /// Caller should spawn `docker::inspect::fetch_detail(docker, id)`
    /// off-thread and forward the result through `DockerMsg::Inspected`
    /// (04-06). Carries the docker CONTAINER id (NOT the entity id).
    SpawnInspect(String),
    /// Caller cycles its `palette` field to the next preset. The order is
    /// owned by the caller (App / run_kitty) — kept out of `apply_input_action`
    /// so the dispatch surface stays pure (no Palette dep). THEME-04.
    CyclePalette,
}

impl Action {
    /// Map a raw [`KeyEvent`] to an [`Action`].
    ///
    /// `Press` AND `Repeat` are treated identically (Pitfall E); `Release` is
    /// ignored. Layout-independent arrow keys coexist with WASD chars
    /// (Pitfall D). Ctrl-C is hard-coded to Quit regardless of key code.
    pub fn from_key(key: KeyEvent) -> Action {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Action::None;
        }
        // Ctrl-C always quits.
        if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Esc => Action::CloseDetail, // dispatch resolves quit vs close
            KeyCode::Left | KeyCode::Char('a') => Action::NudgeYaw(-camera::manual::YAW_STEP),
            KeyCode::Right | KeyCode::Char('d') => Action::NudgeYaw(camera::manual::YAW_STEP),
            KeyCode::Up | KeyCode::Char('w') => Action::NudgePitch(camera::manual::PITCH_STEP),
            KeyCode::Down | KeyCode::Char('s') => Action::NudgePitch(-camera::manual::PITCH_STEP),
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::PageUp => {
                Action::NudgeZoom(-camera::manual::ZOOM_STEP)
            }
            KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::PageDown => {
                Action::NudgeZoom(camera::manual::ZOOM_STEP)
            }
            KeyCode::Tab => Action::SelectNext,
            KeyCode::BackTab => Action::SelectPrev,
            KeyCode::Enter => Action::OpenDetail,
            KeyCode::Char('p') | KeyCode::Char('P') => Action::CyclePalette,
            _ => Action::None,
        }
    }
}

/// Collapse a backlog of queued [`Action`]s into a minimal equivalent set.
///
/// **Why this exists (the post-05-04 release feedback fix).** Terminals deliver
/// key-repeat events at the OS auto-repeat rate (~30 Hz). When the render loop
/// falls behind for a moment (heavy frame, GC pause, scheduler hiccup), repeats
/// queue up in the unbounded event channel. Without coalescing, the loop would
/// drain them one-by-one AFTER the user has released the key — the camera
/// keeps rotating / the palette keeps cycling for a noticeable beat past
/// release. The user observable: "I let go but it kept going for a second".
///
/// The contract is: hold-to-glide still works (each frame still sees at most
/// one nudge of each axis, regardless of backlog depth), but the moment the
/// user releases the key, the loop runs out of events on the NEXT drain pass
/// and motion stops within one frame. Backlog accumulation is incapable of
/// outliving the release because each drain pass throws away duplicates.
///
/// **Coalesce rules:**
///
/// - `NudgeYaw(d)` / `NudgePitch(d)` / `NudgeZoom(d)`: combined into ONE per
///   axis whose delta is the SUM of all queued deltas of that axis. Summing
///   (rather than "last wins") preserves the small-step intent — a queued
///   left+right pair cancels out, which is what the user would expect if they
///   tapped both keys in the same frame. Clamps inside `Camera::nudge_*`
///   handle large summed deltas safely (radius/pitch hard-clamp; yaw wraps).
///
/// - `CyclePalette`, `SelectNext`, `SelectPrev`, `OpenDetail`, `CloseDetail`,
///   `Quit`: kept at most ONCE in the output. These are discrete state-
///   change intents — pressing P 30 times in a single drain pass clearly
///   means "cycle the palette once", not "cycle 30 times".
///
/// - `Action::None` is dropped (carries no intent).
///
/// **Ordering:** the output preserves first-occurrence order across distinct
/// action kinds so the camera flips before selection moves before quit (matches
/// the natural sequential UX). Within a single kind, only one survives.
///
/// **Pure / total / side-effect-free** — safe to unit-test without a Camera
/// or LiveWorld; the coalescing decision is purely on the Action slice.
pub fn coalesce_actions(actions: &[Action]) -> Vec<Action> {
    use std::collections::HashSet;
    use std::mem::discriminant;

    let mut out: Vec<Action> = Vec::with_capacity(actions.len().min(8));
    let mut seen_discrete: HashSet<std::mem::Discriminant<Action>> = HashSet::new();
    // Accumulators for the three nudge axes — index into `out` so we mutate the
    // already-inserted Action in place when more of the same axis arrive.
    let mut yaw_slot: Option<usize> = None;
    let mut pitch_slot: Option<usize> = None;
    let mut zoom_slot: Option<usize> = None;

    for &a in actions {
        match a {
            Action::None => continue,
            Action::NudgeYaw(d) => match yaw_slot {
                Some(i) => {
                    if let Action::NudgeYaw(prev) = out[i] {
                        out[i] = Action::NudgeYaw(prev + d);
                    }
                }
                None => {
                    yaw_slot = Some(out.len());
                    out.push(Action::NudgeYaw(d));
                }
            },
            Action::NudgePitch(d) => match pitch_slot {
                Some(i) => {
                    if let Action::NudgePitch(prev) = out[i] {
                        out[i] = Action::NudgePitch(prev + d);
                    }
                }
                None => {
                    pitch_slot = Some(out.len());
                    out.push(Action::NudgePitch(d));
                }
            },
            Action::NudgeZoom(d) => match zoom_slot {
                Some(i) => {
                    if let Action::NudgeZoom(prev) = out[i] {
                        out[i] = Action::NudgeZoom(prev + d);
                    }
                }
                None => {
                    zoom_slot = Some(out.len());
                    out.push(Action::NudgeZoom(d));
                }
            },
            // Discrete intents: keep at most one of each in the output.
            Action::Quit
            | Action::SelectNext
            | Action::SelectPrev
            | Action::OpenDetail
            | Action::CloseDetail
            | Action::CyclePalette => {
                if seen_discrete.insert(discriminant(&a)) {
                    out.push(a);
                }
            }
        }
    }
    out
}

/// Apply an [`Action`] to the shared input state owned by both backends.
///
/// Returns an [`Effect`] the caller interprets — Quit / SpawnInspect / None.
/// This function is PURE on `(&mut Camera, &mut Selection, &World, &LiveWorld)`:
/// it never touches docker / tokio / spawn. Effects defer those to the caller
/// so the dispatch surface stays trivially testable and bollard-free.
///
/// The moment a Nudge* / Select* / OpenDetail Action arrives, the camera
/// flips to manual mode (CAM-03): the user is now driving. There is no
/// auto-revert in v1.
pub fn apply_input_action(
    action: Action,
    camera: &mut Camera,
    selection: &mut Selection,
    world: &World,
    live: &LiveWorld,
) -> Effect {
    match action {
        Action::Quit => Effect::Quit,
        Action::None => Effect::None,
        Action::NudgeYaw(d) => {
            camera.on_user_input();
            camera.nudge_yaw(d);
            Effect::None
        }
        Action::NudgePitch(d) => {
            camera.on_user_input();
            camera.nudge_pitch(d);
            Effect::None
        }
        Action::NudgeZoom(d) => {
            camera.on_user_input();
            camera.nudge_zoom(d);
            Effect::None
        }
        Action::SelectNext => {
            camera.on_user_input();
            selection.next(world);
            Effect::None
        }
        Action::SelectPrev => {
            camera.on_user_input();
            selection.prev(world);
            Effect::None
        }
        Action::OpenDetail => {
            // Only fire SpawnInspect if there's a selection AND we can resolve
            // the entity id back to a container id string (04-06 needs the
            // container id for inspect_container). Both gates are silent
            // no-ops — pressing Enter with nothing selected just doesn't open
            // a panel.
            let Some(eid) = selection.selected_id else {
                return Effect::None;
            };
            let Some(container_id) = live.id_string_for_entity(eid) else {
                return Effect::None;
            };
            selection.detail_open = true;
            Effect::SpawnInspect(container_id.to_string())
        }
        Action::CloseDetail => {
            if selection.detail_open {
                selection.detail_open = false;
                Effect::None
            } else {
                // Esc with no popup open = quit, matches PROJECT.md's quit set.
                Effect::Quit
            }
        }
        Action::CyclePalette => {
            // Palette cycling is a pure visual swap — does NOT count as
            // "user is driving the camera". Do NOT flip autopilot off, unlike
            // SelectNext/Nudge*. Theme is meta-state, not scene state.
            Effect::CyclePalette
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::manual::{MAX_RADIUS, MIN_RADIUS, PITCH_STEP, YAW_STEP, ZOOM_STEP};
    use crate::docker::ContainerSnapshot;
    use crate::theme::Status;
    use crate::world::live::DockerMsg;
    use crate::world::scene::SceneBounds;
    use crate::world::Entity;
    use glam::Vec3;

    fn key(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind)
    }

    fn key_press(code: KeyCode) -> KeyEvent {
        key(code, KeyEventKind::Press)
    }

    fn key_repeat(code: KeyCode) -> KeyEvent {
        key(code, KeyEventKind::Repeat)
    }

    fn key_release(code: KeyCode) -> KeyEvent {
        key(code, KeyEventKind::Release)
    }

    /// q / Esc / Ctrl-C continue to quit (REND-05 carry-over). Esc maps to
    /// CloseDetail; the apply layer flips it to Quit when no popup is open.
    #[test]
    fn quit_keys_map_to_quit() {
        assert_eq!(Action::from_key(key_press(KeyCode::Char('q'))), Action::Quit);
        let ctrl_c = KeyEvent::new_with_kind(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        );
        assert_eq!(Action::from_key(ctrl_c), Action::Quit);
        // Esc maps to CloseDetail at the from_key level; apply resolves quit.
        assert_eq!(Action::from_key(key_press(KeyCode::Esc)), Action::CloseDetail);
    }

    /// Layout-independent arrows + WASD coexist (Pitfall D).
    #[test]
    fn arrows_and_wasd_map_to_same_nudges() {
        assert_eq!(
            Action::from_key(key_press(KeyCode::Left)),
            Action::NudgeYaw(-YAW_STEP)
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Char('a'))),
            Action::NudgeYaw(-YAW_STEP)
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Right)),
            Action::NudgeYaw(YAW_STEP)
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Char('d'))),
            Action::NudgeYaw(YAW_STEP)
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Up)),
            Action::NudgePitch(PITCH_STEP)
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Char('w'))),
            Action::NudgePitch(PITCH_STEP)
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Down)),
            Action::NudgePitch(-PITCH_STEP)
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Char('s'))),
            Action::NudgePitch(-PITCH_STEP)
        );
    }

    /// All zoom-in spellings collapse to the same NudgeZoom(-ZOOM_STEP).
    #[test]
    fn plus_equals_pageup_all_map_to_zoom_in() {
        let z = Action::NudgeZoom(-ZOOM_STEP);
        assert_eq!(Action::from_key(key_press(KeyCode::Char('+'))), z);
        assert_eq!(Action::from_key(key_press(KeyCode::Char('='))), z);
        assert_eq!(Action::from_key(key_press(KeyCode::PageUp)), z);
    }

    /// All zoom-out spellings collapse to the same NudgeZoom(+ZOOM_STEP).
    #[test]
    fn minus_underscore_pagedown_all_map_to_zoom_out() {
        let z = Action::NudgeZoom(ZOOM_STEP);
        assert_eq!(Action::from_key(key_press(KeyCode::Char('-'))), z);
        assert_eq!(Action::from_key(key_press(KeyCode::Char('_'))), z);
        assert_eq!(Action::from_key(key_press(KeyCode::PageDown)), z);
    }

    /// Tab / BackTab / Enter map to the selection / open-detail intents.
    #[test]
    fn tab_backtab_enter_map_to_selection_intents() {
        assert_eq!(Action::from_key(key_press(KeyCode::Tab)), Action::SelectNext);
        assert_eq!(
            Action::from_key(key_press(KeyCode::BackTab)),
            Action::SelectPrev
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Enter)),
            Action::OpenDetail
        );
    }

    /// Repeat events route the same as Press (Pitfall E).
    #[test]
    fn repeat_kind_maps_same_as_press() {
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Left)),
            Action::NudgeYaw(-YAW_STEP)
        );
        assert_eq!(Action::from_key(key_repeat(KeyCode::Tab)), Action::SelectNext);
        assert_eq!(Action::from_key(key_repeat(KeyCode::Char('q'))), Action::Quit);
    }

    /// Release events are dropped — manual mode is triggered by Press/Repeat.
    #[test]
    fn release_kind_maps_to_none() {
        assert_eq!(
            Action::from_key(key_release(KeyCode::Left)),
            Action::None
        );
        assert_eq!(Action::from_key(key_release(KeyCode::Tab)), Action::None);
    }

    // ---- apply_input_action / Effect tests ---------------------------------

    fn world_with_one_entity(id: u32) -> World {
        let entities = vec![Entity {
            id,
            position: Vec3::ZERO,
            half_extents: Vec3::splat(0.5),
            status: Status::Running,
            group: 0,
        }];
        let bounds = SceneBounds::from_entities(&entities);
        World { entities, bounds }
    }

    /// Quit action surfaces as `Effect::Quit`.
    #[test]
    fn quit_action_returns_effect_quit() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = World {
            entities: vec![],
            bounds: SceneBounds::from_entities(&[]),
        };
        let live = LiveWorld::new();
        let effect = apply_input_action(Action::Quit, &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::Quit);
    }

    /// NudgeYaw flips autopilot off AND mutates yaw by the delta.
    #[test]
    fn nudge_returns_effect_none_and_flips_autopilot() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = World {
            entities: vec![],
            bounds: SceneBounds::from_entities(&[]),
        };
        let live = LiveWorld::new();
        assert!(cam.autopilot_active);
        let yaw_before = cam.yaw;
        let effect =
            apply_input_action(Action::NudgeYaw(0.1), &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::None);
        assert!(!cam.autopilot_active, "first nudge must flip autopilot off");
        assert!(
            (cam.yaw - (yaw_before + 0.1)).abs() < 1e-5,
            "yaw should have been nudged by +0.1"
        );
    }

    /// NudgeZoom clamps via Camera::nudge_zoom and surfaces None.
    #[test]
    fn nudge_zoom_clamps_via_camera_method() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = World {
            entities: vec![],
            bounds: SceneBounds::from_entities(&[]),
        };
        let live = LiveWorld::new();
        let effect = apply_input_action(
            Action::NudgeZoom(-1000.0),
            &mut cam,
            &mut sel,
            &world,
            &live,
        );
        assert_eq!(effect, Effect::None);
        assert!((cam.radius - MIN_RADIUS).abs() < 1e-6);
        // Push past MAX too.
        apply_input_action(
            Action::NudgeZoom(10_000.0),
            &mut cam,
            &mut sel,
            &world,
            &live,
        );
        assert!((cam.radius - MAX_RADIUS).abs() < 1e-6);
    }

    /// CloseDetail with no panel open returns Effect::Quit (Esc-quit path).
    #[test]
    fn close_detail_with_no_panel_open_returns_effect_quit() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = world_with_one_entity(1);
        let live = LiveWorld::new();
        assert!(!sel.detail_open);
        let effect =
            apply_input_action(Action::CloseDetail, &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::Quit);
    }

    /// CloseDetail with the panel open returns None and closes the panel.
    #[test]
    fn close_detail_with_panel_open_returns_effect_none_and_closes() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        sel.detail_open = true;
        let world = world_with_one_entity(1);
        let live = LiveWorld::new();
        let effect =
            apply_input_action(Action::CloseDetail, &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::None);
        assert!(!sel.detail_open);
    }

    /// OpenDetail with no selection is a no-op (returns Effect::None).
    #[test]
    fn open_detail_no_selection_returns_effect_none() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = world_with_one_entity(1);
        let live = LiveWorld::new();
        assert!(sel.selected_id.is_none());
        let effect =
            apply_input_action(Action::OpenDetail, &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::None);
        assert!(!sel.detail_open);
    }

    /// OpenDetail with a selection AND a known live container id string
    /// surfaces Effect::SpawnInspect(container_id) and sets detail_open=true.
    #[test]
    fn open_detail_with_selection_returns_effect_spawn_inspect_with_id() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let mut live = LiveWorld::new();
        // Seed the live world with a known id; the first Added of a fresh
        // group gets entity_id = (0 << 16) | 0 = 0.
        live.apply(DockerMsg::Added(ContainerSnapshot {
            id: "abc123".to_string(),
            name: "x".to_string(),
            status: Status::Running,
            group_key: "net0".to_string(),
            ..ContainerSnapshot::default()
        }));
        let world = world_with_one_entity(0);
        sel.selected_id = Some(0);
        let effect =
            apply_input_action(Action::OpenDetail, &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::SpawnInspect("abc123".to_string()));
        assert!(sel.detail_open, "OpenDetail must set detail_open=true");
    }

    /// SelectNext flips autopilot off (interacting = driving) and advances
    /// the selection.
    #[test]
    fn select_next_flips_autopilot_and_advances_selection() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = world_with_one_entity(42);
        let live = LiveWorld::new();
        assert!(cam.autopilot_active);
        let effect =
            apply_input_action(Action::SelectNext, &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::None);
        assert!(!cam.autopilot_active);
        assert_eq!(sel.selected_id, Some(42));
    }

    // ---- THEME-04 (05-04) CyclePalette key + dispatch -----------------------

    /// 'p' and 'P' both map to Action::CyclePalette (shift-forgiveness,
    /// consistent with Tab / BackTab style coverage).
    #[test]
    fn p_key_maps_to_cycle_palette() {
        assert_eq!(
            Action::from_key(key_press(KeyCode::Char('p'))),
            Action::CyclePalette
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Char('P'))),
            Action::CyclePalette
        );
    }

    /// CyclePalette dispatches to Effect::CyclePalette — the caller
    /// (App / run_kitty) owns the cycle order and the actual palette swap.
    #[test]
    fn apply_cycle_palette_returns_effect_cycle_palette() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = World {
            entities: vec![],
            bounds: SceneBounds::from_entities(&[]),
        };
        let live = LiveWorld::new();
        let effect =
            apply_input_action(Action::CyclePalette, &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::CyclePalette);
    }

    /// Palette cycling is meta-state, NOT scene state — unlike Nudge* /
    /// Select*, it must NOT flip the camera out of autopilot mode.
    #[test]
    fn apply_cycle_palette_does_not_flip_autopilot() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = World {
            entities: vec![],
            bounds: SceneBounds::from_entities(&[]),
        };
        let live = LiveWorld::new();
        assert!(cam.autopilot_active);
        let _ = apply_input_action(Action::CyclePalette, &mut cam, &mut sel, &world, &live);
        assert!(
            cam.autopilot_active,
            "CyclePalette is meta-state and must NOT disturb autopilot mode"
        );
    }

    // ---- 05-04-RV1: coalesce_actions — release-stops-input regression -------

    /// 30 queued CyclePalette repeats (the OS key-repeat backlog after holding
    /// P for a second) coalesce to exactly ONE CyclePalette in the output.
    /// This is the load-bearing pin for the "release stops palette cycling"
    /// fix.
    #[test]
    fn coalesce_collapses_n_cycle_palette_into_one() {
        let backlog: Vec<Action> = std::iter::repeat_n(Action::CyclePalette, 30).collect();
        let collapsed = coalesce_actions(&backlog);
        assert_eq!(
            collapsed,
            vec![Action::CyclePalette],
            "30 queued P repeats must collapse to exactly one CyclePalette"
        );
    }

    /// Multiple queued NudgeYaw deltas SUM into one. Sum (not last-wins)
    /// preserves the small-step UX — opposite-direction nudges in the same
    /// drain pass cancel cleanly.
    #[test]
    fn coalesce_sums_nudge_yaw_deltas() {
        let backlog = vec![
            Action::NudgeYaw(0.1),
            Action::NudgeYaw(0.1),
            Action::NudgeYaw(-0.05),
        ];
        let collapsed = coalesce_actions(&backlog);
        assert_eq!(collapsed.len(), 1);
        match collapsed[0] {
            Action::NudgeYaw(d) => assert!(
                (d - 0.15).abs() < 1e-5,
                "summed yaw delta should be 0.1 + 0.1 - 0.05 = 0.15, got {d}"
            ),
            _ => panic!("expected NudgeYaw, got {:?}", collapsed[0]),
        }
    }

    /// Each axis is coalesced independently — a backlog of W (pitch) + D
    /// (yaw) repeats collapses to one of each, not one combined nudge. The
    /// user's pitch and yaw inputs are orthogonal.
    #[test]
    fn coalesce_keeps_separate_axes() {
        let backlog = vec![
            Action::NudgeYaw(0.1),
            Action::NudgePitch(0.07),
            Action::NudgeYaw(0.1),
            Action::NudgePitch(0.07),
            Action::NudgeYaw(0.1),
        ];
        let collapsed = coalesce_actions(&backlog);
        assert_eq!(collapsed.len(), 2);
        // Yaw came first, so it appears first in the output (first-occurrence
        // order across kinds).
        match collapsed[0] {
            Action::NudgeYaw(d) => assert!((d - 0.3).abs() < 1e-5),
            _ => panic!("expected NudgeYaw first"),
        }
        match collapsed[1] {
            Action::NudgePitch(d) => assert!((d - 0.14).abs() < 1e-5),
            _ => panic!("expected NudgePitch second"),
        }
    }

    /// SelectNext repeats from holding Tab collapse to ONE SelectNext per
    /// drain. The user wants Tab to step once per press, not Nx per release-
    /// drain.
    #[test]
    fn coalesce_collapses_select_next_repeats() {
        let backlog: Vec<Action> = std::iter::repeat_n(Action::SelectNext, 20).collect();
        let collapsed = coalesce_actions(&backlog);
        assert_eq!(collapsed, vec![Action::SelectNext]);
    }

    /// Action::None entries are dropped — they carry no intent and clutter
    /// the output.
    #[test]
    fn coalesce_drops_none_entries() {
        let backlog = vec![
            Action::None,
            Action::NudgeYaw(0.1),
            Action::None,
            Action::None,
        ];
        let collapsed = coalesce_actions(&backlog);
        assert_eq!(collapsed, vec![Action::NudgeYaw(0.1)]);
    }

    /// Empty input produces empty output (no-op safety pin).
    #[test]
    fn coalesce_empty_input_is_empty() {
        assert!(coalesce_actions(&[]).is_empty());
    }

    /// A mixed backlog (yaw repeats + palette repeats + select repeats) all
    /// collapse simultaneously. This is the realistic case when the user
    /// holds multiple keys briefly (e.g. P then Tab then W).
    #[test]
    fn coalesce_mixed_backlog_collapses_all() {
        let backlog = vec![
            Action::CyclePalette,
            Action::NudgeYaw(0.1),
            Action::CyclePalette,
            Action::SelectNext,
            Action::NudgeYaw(0.1),
            Action::SelectNext,
            Action::CyclePalette,
        ];
        let collapsed = coalesce_actions(&backlog);
        assert_eq!(collapsed.len(), 3, "expected 3 distinct kinds: {collapsed:?}");
        // CyclePalette appears first (it was first in input).
        assert_eq!(collapsed[0], Action::CyclePalette);
        // NudgeYaw second with summed delta.
        match collapsed[1] {
            Action::NudgeYaw(d) => assert!((d - 0.2).abs() < 1e-5),
            _ => panic!("expected NudgeYaw"),
        }
        // SelectNext third.
        assert_eq!(collapsed[2], Action::SelectNext);
    }

    /// Quit is also coalesced — Ctrl-C + q in the same drain should fire once
    /// (no harm, but documents the rule).
    #[test]
    fn coalesce_collapses_quit_repeats() {
        let backlog = vec![Action::Quit, Action::Quit, Action::Quit];
        assert_eq!(coalesce_actions(&backlog), vec![Action::Quit]);
    }
}
