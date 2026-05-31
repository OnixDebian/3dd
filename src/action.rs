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
//! | 'l' / 'L'                           | ToggleLegend                    |
//!
//! Pitfall D: `'a'` and `Left` (and the other WASD/arrow pairs) coexist so
//! AZERTY/Dvorak users still get layout-independent input via arrows.
//!
//! Pitfall E: `KeyEventKind::Press` AND `KeyEventKind::Repeat` are treated
//! identically for CONTINUOUS Nudge axes (the OS-repeat fallback path on
//! terminals without the kitty keyboard protocol). For DISCRETE actions
//! (CyclePalette / ToggleLegend / SelectNext / SelectPrev / OpenDetail /
//! CloseDetail / Quit) only `Press` produces an Action — `Repeat` and
//! `Release` map to `Action::None`. This is the 05-05-RV7 "kill the
//! OS-repeat delay" fix: holding `P` MUST cycle the palette once (not
//! N times), and holding an arrow drives continuous nudges (via either
//! KKP held-set tracking — see [`HeldAction`] — or OS Repeat fallback
//! depending on terminal capability).
//!
//! ## 05-05-RV7: KKP-driven held-key tracking ([`HeldAction`])
//!
//! User feedback after 05-05-RV6: "когда зажмию стрелки первый тик
//! срабатывает сразу, а второй с задержкой, можно её убрать (при
//! повороте камеры или смене темы)". Translated: holding an arrow key
//! fires the first tick immediately, then waits the OS auto-repeat
//! delay (~250-500 ms) before the second tick — the user perceives
//! input lag.
//!
//! Root cause: terminals deliver key-repeat events at the OS-configured
//! cadence with the OS-configured initial delay. With ONLY Press events
//! available (most terminals' default), we can't track "is the key still
//! held" — we are stuck with the OS-repeat rhythm including its initial
//! pause.
//!
//! Fix: enable the kitty keyboard protocol (KKP) at `Tui::enter` /
//! `run_kitty` startup; KKP delivers `KeyEventKind::Press` AND
//! `KeyEventKind::Release` AND `KeyEventKind::Repeat`. The render loop
//! maintains a `HashSet<HeldAction>`: insert on Press, remove on
//! Release. Each render tick, if any held action is present, dispatch
//! ONE nudge per held action — bypassing the OS initial-delay entirely.
//! Discrete actions are NOT auto-fired on Repeat (they fire once per
//! Press), so holding `P` cycles the palette exactly once.
//!
//! On terminals without KKP support, `kkp_active` stays false and the
//! held-set stays empty; the OS-repeat → Action::Nudge* fallback path
//! (Press + Repeat both produce Nudge Actions, coalesced once per
//! frame) preserves the pre-RV7 behavior. So the worst case is "no
//! regression"; the best case (kitty / ghostty / WezTerm) is the lag
//! is gone.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::camera::{self, Camera};
use crate::world::live::LiveWorld;
use crate::world::selection::Selection;
use crate::world::World;

/// A continuous movement intent that can be HELD across multiple render
/// ticks (05-05-RV7). When the kitty keyboard protocol (KKP) is active,
/// the render loop tracks a `HashSet<HeldAction>` — insert on Press,
/// remove on Release — and emits one nudge per held action per tick.
/// This sidesteps the OS auto-repeat initial-delay (~250-500 ms) that
/// caused the user-reported "first tick fires immediately but second
/// has a delay" perception.
///
/// Discrete actions (Quit, CyclePalette, ToggleLegend, Tab/BackTab,
/// Enter, Esc) are NOT held — they fire once per Press and ignore
/// Repeat/Release. Holding `P` cycles the palette ONCE; holding an
/// arrow continuously orbits the camera. The two intent classes are
/// kept separate by this enum (continuous → HeldAction) vs the rest
/// of [`Action`] (discrete → Press-only).
// The shared `Nudge` prefix on the variants below is intentional —
// these are the held-continuous-nudge intents, and the prefix makes
// the call sites read naturally (e.g.
// `HeldAction::NudgeYawLeft.to_action()` returns `Action::NudgeYaw(...)`,
// pinning the parallelism between held intents and dispatched Actions).
// Removing the prefix would just move the `Nudge` into the call site
// path without improving anything.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeldAction {
    /// Left arrow / 'a' — orbit yaw left.
    NudgeYawLeft,
    /// Right arrow / 'd' — orbit yaw right.
    NudgeYawRight,
    /// Up arrow / 'w' — orbit pitch up.
    NudgePitchUp,
    /// Down arrow / 's' — orbit pitch down.
    NudgePitchDown,
    /// '+' / '=' / PageUp — zoom in (closer).
    NudgeZoomIn,
    /// '-' / '_' / PageDown — zoom out (farther).
    NudgeZoomOut,
}

impl HeldAction {
    /// Map a raw [`KeyCode`] to a held continuous-movement action, if any.
    /// `None` for discrete keys (P, L, Tab, Enter, q, Esc, …) — those are
    /// NOT held; they fire once per Press via [`Action::from_key`].
    ///
    /// The set of mapped key codes mirrors the continuous arms of
    /// [`Action::from_key`] exactly. Layout-independent arrows + WASD
    /// coexist for Pitfall D coverage.
    pub fn from_key_code(code: KeyCode) -> Option<HeldAction> {
        match code {
            KeyCode::Left | KeyCode::Char('a') => Some(HeldAction::NudgeYawLeft),
            KeyCode::Right | KeyCode::Char('d') => Some(HeldAction::NudgeYawRight),
            KeyCode::Up | KeyCode::Char('w') => Some(HeldAction::NudgePitchUp),
            KeyCode::Down | KeyCode::Char('s') => Some(HeldAction::NudgePitchDown),
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::PageUp => {
                Some(HeldAction::NudgeZoomIn)
            }
            KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::PageDown => {
                Some(HeldAction::NudgeZoomOut)
            }
            _ => None,
        }
    }

    /// The [`Action`] this held intent dispatches one of per render tick.
    /// Reads the same step constants the OS-repeat fallback uses, so
    /// hold-cadence under KKP matches hold-cadence on non-KKP terminals
    /// to within the difference between (OS repeat rate) and (render
    /// rate). Both ~30 Hz on a typical Linux setup.
    pub fn to_action(self) -> Action {
        match self {
            HeldAction::NudgeYawLeft => Action::NudgeYaw(-camera::manual::YAW_STEP),
            HeldAction::NudgeYawRight => Action::NudgeYaw(camera::manual::YAW_STEP),
            HeldAction::NudgePitchUp => Action::NudgePitch(camera::manual::PITCH_STEP),
            HeldAction::NudgePitchDown => Action::NudgePitch(-camera::manual::PITCH_STEP),
            HeldAction::NudgeZoomIn => Action::NudgeZoom(-camera::manual::ZOOM_STEP),
            HeldAction::NudgeZoomOut => Action::NudgeZoom(camera::manual::ZOOM_STEP),
        }
    }
}

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
    /// Toggle the legend HUD overlay on/off (L key). THEME-05.
    ToggleLegend,
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
    /// Caller flips its `hud_visible` field — the legend HUD overlay
    /// toggles on/off. Like `CyclePalette`, this is meta-state, not scene
    /// state — does NOT flip autopilot. THEME-05.
    ToggleLegend,
}

impl Action {
    /// Map a raw [`KeyEvent`] to an [`Action`].
    ///
    /// For CONTINUOUS Nudge* axes: `Press` AND `Repeat` are treated
    /// identically (Pitfall E) — this preserves the OS-repeat fallback
    /// path for terminals without KKP (Press once → first nudge; OS
    /// auto-repeats → subsequent nudges at the OS cadence).
    ///
    /// For DISCRETE actions (Quit, CloseDetail, SelectNext/Prev,
    /// OpenDetail, CyclePalette, ToggleLegend): ONLY `Press` is honored.
    /// `Repeat` and `Release` map to `Action::None`. This is the
    /// 05-05-RV7 contract: holding `P` cycles the palette ONCE (not
    /// N times at the OS-repeat rate), holding `Tab` advances selection
    /// ONCE per press, etc. Discrete intents are "edge-triggered" — they
    /// don't auto-fire while held.
    ///
    /// `Release` is also dropped for continuous keys at this layer; the
    /// KKP-aware render loop consumes Release events via
    /// [`HeldAction::from_key_code`] BEFORE calling `from_key`, so by
    /// the time a Release would reach this function it has already
    /// been processed into the held-set removal.
    ///
    /// Layout-independent arrow keys coexist with WASD chars
    /// (Pitfall D). Ctrl-C is hard-coded to Quit regardless of key code
    /// (and only on Press, same rule).
    pub fn from_key(key: KeyEvent) -> Action {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Action::None;
        }
        let is_repeat = matches!(key.kind, KeyEventKind::Repeat);
        // Ctrl-C always quits — Press only (no auto-quit on hold).
        if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
            if is_repeat {
                return Action::None;
            }
            return Action::Quit;
        }
        match key.code {
            // ---- DISCRETE: Press-only; Repeat → None so holding doesn't fire N times ----
            KeyCode::Char('q') => {
                if is_repeat {
                    Action::None
                } else {
                    Action::Quit
                }
            }
            KeyCode::Esc => {
                if is_repeat {
                    Action::None
                } else {
                    Action::CloseDetail
                }
            }
            KeyCode::Tab => {
                if is_repeat {
                    Action::None
                } else {
                    Action::SelectNext
                }
            }
            KeyCode::BackTab => {
                if is_repeat {
                    Action::None
                } else {
                    Action::SelectPrev
                }
            }
            KeyCode::Enter => {
                if is_repeat {
                    Action::None
                } else {
                    Action::OpenDetail
                }
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if is_repeat {
                    Action::None
                } else {
                    Action::CyclePalette
                }
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                if is_repeat {
                    Action::None
                } else {
                    Action::ToggleLegend
                }
            }
            // ---- CONTINUOUS: Press AND Repeat both produce a Nudge (Pitfall E) ----
            // These remain the OS-repeat fallback path for non-KKP terminals.
            // On KKP-active terminals the render loop ALSO uses these via
            // `HeldAction::to_action()` to drive nudges every tick a key is in
            // the held set — but the KKP loop ignores incoming Repeat events
            // for movement keys (it tracks Press/Release into the held-set
            // instead). The map below stays unchanged so the legacy
            // OS-repeat path is preserved byte-for-byte.
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
/// - `CyclePalette`, `ToggleLegend`, `SelectNext`, `SelectPrev`,
///   `OpenDetail`, `CloseDetail`, `Quit`: kept at most ONCE in the output.
///   These are discrete state-change intents — pressing P 30 times in a
///   single drain pass clearly means "cycle the palette once", not "cycle
///   30 times". Same logic applies to holding L: one toggle per drain,
///   never N toggles that net to the wrong parity.
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
            | Action::CyclePalette
            | Action::ToggleLegend => {
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
        Action::ToggleLegend => {
            // Same rule as CyclePalette: HUD toggle is meta-state, does NOT
            // flip autopilot. THEME-05.
            Effect::ToggleLegend
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

    /// Repeat events route the same as Press for CONTINUOUS Nudge axes
    /// (Pitfall E — preserves OS-repeat fallback on non-KKP terminals).
    /// But for DISCRETE actions (Tab, q, Esc, P, L, Enter), Repeat maps to
    /// `Action::None` (05-05-RV7 — holding `P` must cycle ONCE, not N
    /// times). The held-set tracking in the KKP-aware render loop drives
    /// continuous nudges directly from Press/Release, bypassing this
    /// Repeat path entirely on supported terminals.
    #[test]
    fn repeat_kind_maps_same_as_press_for_continuous_nudges() {
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Left)),
            Action::NudgeYaw(-YAW_STEP),
            "Left arrow Repeat must still produce NudgeYaw — OS-repeat fallback"
        );
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Char('w'))),
            Action::NudgePitch(PITCH_STEP),
            "W key Repeat must still produce NudgePitch — OS-repeat fallback"
        );
    }

    /// 05-05-RV7: holding a DISCRETE key (Tab / q / Esc / Enter / P / L)
    /// must NOT auto-fire on OS-repeat. The render loop fires the
    /// discrete intent once per Press; Repeat is dropped at this layer.
    #[test]
    fn repeat_kind_drops_discrete_actions() {
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Tab)),
            Action::None,
            "holding Tab must NOT advance selection N times — discrete"
        );
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::BackTab)),
            Action::None,
            "holding Shift-Tab must NOT step selection N times — discrete"
        );
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Char('q'))),
            Action::None,
            "holding q must NOT spam Quit — discrete (one-shot quit)"
        );
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Esc)),
            Action::None,
            "holding Esc must NOT spam CloseDetail/Quit — discrete"
        );
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Enter)),
            Action::None,
            "holding Enter must NOT auto-open detail N times — discrete"
        );
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Char('p'))),
            Action::None,
            "holding P must NOT auto-cycle palette N times — discrete (RV7 fix)"
        );
        assert_eq!(
            Action::from_key(key_repeat(KeyCode::Char('l'))),
            Action::None,
            "holding L must NOT auto-toggle HUD N times — discrete (RV7 fix)"
        );
    }

    /// 05-05-RV7: Ctrl-C only quits on Press, not Repeat (no spam on hold).
    #[test]
    fn ctrl_c_repeat_drops_to_none() {
        let ctrl_c_repeat = KeyEvent::new_with_kind(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        );
        assert_eq!(Action::from_key(ctrl_c_repeat), Action::None);
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

    // ---- THEME-05 (05-05) ToggleLegend key + dispatch -----------------------

    /// 'l' and 'L' both map to Action::ToggleLegend (shift-forgiveness,
    /// matches the 'p'/'P' CyclePalette pattern).
    #[test]
    fn l_key_maps_to_toggle_legend() {
        assert_eq!(
            Action::from_key(key_press(KeyCode::Char('l'))),
            Action::ToggleLegend
        );
        assert_eq!(
            Action::from_key(key_press(KeyCode::Char('L'))),
            Action::ToggleLegend
        );
    }

    /// ToggleLegend dispatches to Effect::ToggleLegend — the caller
    /// (App / run_kitty) owns the actual `hud_visible` flip.
    #[test]
    fn apply_toggle_legend_returns_effect_toggle_legend() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = World {
            entities: vec![],
            bounds: SceneBounds::from_entities(&[]),
        };
        let live = LiveWorld::new();
        let effect =
            apply_input_action(Action::ToggleLegend, &mut cam, &mut sel, &world, &live);
        assert_eq!(effect, Effect::ToggleLegend);
    }

    /// HUD toggle is meta-state, NOT scene state — it must NOT flip the
    /// camera out of autopilot mode. Mirrors the CyclePalette pin.
    #[test]
    fn apply_toggle_legend_does_not_flip_autopilot() {
        let mut cam = Camera::new();
        let mut sel = Selection::new();
        let world = World {
            entities: vec![],
            bounds: SceneBounds::from_entities(&[]),
        };
        let live = LiveWorld::new();
        assert!(cam.autopilot_active);
        let _ = apply_input_action(Action::ToggleLegend, &mut cam, &mut sel, &world, &live);
        assert!(
            cam.autopilot_active,
            "ToggleLegend is meta-state and must NOT disturb autopilot mode"
        );
    }

    /// 30 queued ToggleLegend repeats (OS auto-repeat backlog while holding L)
    /// coalesce to exactly ONE — so a brief hold doesn't toggle the HUD off
    /// and on N/2 times. Same contract as CyclePalette.
    #[test]
    fn coalesce_collapses_n_toggle_legend_into_one() {
        let backlog: Vec<Action> = std::iter::repeat_n(Action::ToggleLegend, 30).collect();
        let collapsed = coalesce_actions(&backlog);
        assert_eq!(
            collapsed,
            vec![Action::ToggleLegend],
            "30 queued L repeats must collapse to exactly one ToggleLegend"
        );
    }

    // ---- 05-05-RV7: HeldAction tracking for KKP-driven continuous nudges ----

    /// All six movement keys (Left/Right/Up/Down/+/-) map to held actions.
    /// Layout-independent arrows and WASD coexist (same Pitfall D contract
    /// as `Action::from_key`).
    #[test]
    fn held_action_covers_all_movement_keys() {
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Left),
            Some(HeldAction::NudgeYawLeft)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Char('a')),
            Some(HeldAction::NudgeYawLeft)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Right),
            Some(HeldAction::NudgeYawRight)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Char('d')),
            Some(HeldAction::NudgeYawRight)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Up),
            Some(HeldAction::NudgePitchUp)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Char('w')),
            Some(HeldAction::NudgePitchUp)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Down),
            Some(HeldAction::NudgePitchDown)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Char('s')),
            Some(HeldAction::NudgePitchDown)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Char('+')),
            Some(HeldAction::NudgeZoomIn)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Char('=')),
            Some(HeldAction::NudgeZoomIn)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::PageUp),
            Some(HeldAction::NudgeZoomIn)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Char('-')),
            Some(HeldAction::NudgeZoomOut)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::Char('_')),
            Some(HeldAction::NudgeZoomOut)
        );
        assert_eq!(
            HeldAction::from_key_code(KeyCode::PageDown),
            Some(HeldAction::NudgeZoomOut)
        );
    }

    /// Discrete keys (P, L, Tab, Enter, q, Esc) MUST return None — they
    /// are not held, they fire once per Press. Pinning this guarantees a
    /// future refactor can't accidentally start auto-cycling the palette
    /// while P is held.
    #[test]
    fn held_action_skips_discrete_keys() {
        for code in [
            KeyCode::Char('p'),
            KeyCode::Char('P'),
            KeyCode::Char('l'),
            KeyCode::Char('L'),
            KeyCode::Char('q'),
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Enter,
            KeyCode::Esc,
        ] {
            assert_eq!(
                HeldAction::from_key_code(code),
                None,
                "discrete key {code:?} must NOT map to HeldAction — discrete actions fire once per Press, not continuously"
            );
        }
    }

    /// Each HeldAction dispatches to the SAME `Action` shape the OS-repeat
    /// fallback path produces from the corresponding key. Pins parity:
    /// hold-cadence under KKP must match hold-cadence on non-KKP terminals
    /// (to within the OS-repeat-rate vs render-rate difference).
    #[test]
    fn held_action_to_action_matches_from_key() {
        // Left arrow: HeldAction::NudgeYawLeft.to_action() must equal
        // Action::from_key(Left Press).
        assert_eq!(
            HeldAction::NudgeYawLeft.to_action(),
            Action::from_key(key_press(KeyCode::Left))
        );
        assert_eq!(
            HeldAction::NudgeYawRight.to_action(),
            Action::from_key(key_press(KeyCode::Right))
        );
        assert_eq!(
            HeldAction::NudgePitchUp.to_action(),
            Action::from_key(key_press(KeyCode::Up))
        );
        assert_eq!(
            HeldAction::NudgePitchDown.to_action(),
            Action::from_key(key_press(KeyCode::Down))
        );
        assert_eq!(
            HeldAction::NudgeZoomIn.to_action(),
            Action::from_key(key_press(KeyCode::Char('+')))
        );
        assert_eq!(
            HeldAction::NudgeZoomOut.to_action(),
            Action::from_key(key_press(KeyCode::Char('-')))
        );
    }

    /// 05-05-RV7 simulation: 5 render ticks with Left arrow held drives
    /// EXACTLY 5 nudges (no OS-delay gap). This is the held-set model:
    /// the render loop emits one `to_action()` dispatch per held entry
    /// per tick, bypassing the OS auto-repeat initial delay that caused
    /// the user-reported lag.
    #[test]
    fn held_action_drives_one_nudge_per_render_tick() {
        use std::collections::HashSet;
        let mut held: HashSet<HeldAction> = HashSet::new();
        // User presses Left — KKP delivers Press; loop inserts into set.
        held.insert(HeldAction::NudgeYawLeft);
        // 5 render ticks: each emits one Action::NudgeYaw(-YAW_STEP).
        let nudges_per_tick: Vec<Action> = (0..5)
            .flat_map(|_| {
                // The render loop drains the held set into actions:
                held.iter().map(|h| h.to_action()).collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            nudges_per_tick.len(),
            5,
            "5 render ticks with Left held must emit exactly 5 nudges, not 1+OS-delayed-4"
        );
        for n in &nudges_per_tick {
            assert_eq!(*n, Action::NudgeYaw(-YAW_STEP));
        }

        // User releases Left — KKP delivers Release; loop removes from set.
        held.remove(&HeldAction::NudgeYawLeft);
        // Next 5 ticks: no nudges, motion stops within ONE tick of release.
        let post_release: Vec<Action> = (0..5)
            .flat_map(|_| held.iter().map(|h| h.to_action()).collect::<Vec<_>>())
            .collect();
        assert_eq!(
            post_release.len(),
            0,
            "after Release, motion must stop within 1 tick (held set is empty)"
        );
    }

    /// Multiple held keys (e.g. Left + Up = diagonal orbit) drive ALL of
    /// their nudges every tick — pitch and yaw advance simultaneously.
    /// This is the "natural" KKP UX: diagonal arrows do what they say.
    #[test]
    fn held_action_supports_multiple_simultaneous_keys() {
        use std::collections::HashSet;
        let mut held: HashSet<HeldAction> = HashSet::new();
        held.insert(HeldAction::NudgeYawLeft);
        held.insert(HeldAction::NudgePitchUp);
        let one_tick: Vec<Action> = held.iter().map(|h| h.to_action()).collect();
        assert_eq!(one_tick.len(), 2, "two held keys must produce two nudges per tick");
        assert!(one_tick.contains(&Action::NudgeYaw(-YAW_STEP)));
        assert!(one_tick.contains(&Action::NudgePitch(PITCH_STEP)));
    }

    /// Inserting the same key twice (a stuck Press-without-Release race)
    /// is idempotent — HashSet semantics naturally cap at one per
    /// HeldAction kind. Pin this so future refactors can't accidentally
    /// switch to a Vec<HeldAction> that would double-fire.
    #[test]
    fn held_action_set_is_idempotent_on_duplicate_press() {
        use std::collections::HashSet;
        let mut held: HashSet<HeldAction> = HashSet::new();
        held.insert(HeldAction::NudgeYawLeft);
        held.insert(HeldAction::NudgeYawLeft);
        held.insert(HeldAction::NudgeYawLeft);
        assert_eq!(held.len(), 1, "duplicate Press must not duplicate the held entry");
    }
}
