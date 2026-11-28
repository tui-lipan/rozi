//! Synthetic mouse gesture expansion and server-side wire encoding for `send-mouse`.
//!
//! All protocol framing is delegated to [`tui_lipan::encode_mouse_event`]. Gestures are expanded
//! and encoded in full before any byte is written to the PTY, so a refused step never leaves a
//! half-delivered click or drag in the application.

use tui_lipan::core::event::{MouseButton as LipanMouseButton, MouseEvent, MouseKind};
use tui_lipan::{
    KeyMods, MouseMode, MouseModeState, MouseReportError, MouseReportGeometry, TerminalCellSize,
    encode_mouse_event,
};

use crate::control::{
    ControlErrorCode, ControlResponse, MouseAction, MouseButton, MouseModifiers, MousePoint,
    MouseScrollDirection,
};
use crate::pane::id::PaneId;

/// Encode `action` into the full sequence of wire bytes for pane `pane_id`, or return a refusal
/// describing which part failed and why.
pub fn encode_mouse_action(
    pane_id: PaneId,
    action: &MouseAction,
    cols: u16,
    rows: u16,
    cell: TerminalCellSize,
    mode: MouseModeState,
) -> Result<Vec<u8>, ControlResponse> {
    if let Err(message) = action.validate() {
        return Err(ControlResponse::error_with(
            ControlErrorCode::InvalidArgument,
            message,
        ));
    }

    match *action {
        MouseAction::Click { position, .. }
        | MouseAction::Scroll { position, .. }
        | MouseAction::Move { position, .. } => {
            check_bounds(pane_id, position, cols, rows)?;
        }
        MouseAction::Drag { from, to, .. } => {
            check_bounds(pane_id, from, cols, rows)?;
            check_bounds(pane_id, to, cols, rows)?;
        }
    }

    let geometry = MouseReportGeometry {
        content_origin: (0, 0),
        sub_cell: None,
        cell,
    };
    let events = expand_gesture(action);
    let is_click = matches!(action, MouseAction::Click { .. });

    // A zero-distance drag (`from == to`) has no intermediate cells on the Bresenham path, but it
    // is still a drag gesture and must be refused when the tracking mode does not report drags.
    if let MouseAction::Drag {
        from, to, button, ..
    } = *action
        && from == to
        && mode.mode != MouseMode::None
        && !mode.mode.allows(MouseKind::Drag(to_lipan_button(button)))
    {
        return Err(report_error_response(
            pane_id,
            MouseKind::Drag(to_lipan_button(button)),
            mode,
            MouseReportError::NotReported,
        ));
    }

    let mut bytes = Vec::new();
    for event in events {
        match encode_mouse_event(event, mode, geometry) {
            Ok(forwarded) => bytes.extend_from_slice(&forwarded.bytes),
            // Under X10 compatibility tracking (DEC 9) terminals report presses only; omit the
            // matching release of a click rather than refusing the click itself.
            Err(MouseReportError::NotReported)
                if is_click
                    && mode.mode == MouseMode::X10
                    && matches!(event.kind, MouseKind::Up(_)) => {}
            Err(err) => {
                return Err(report_error_response(pane_id, event.kind, mode, err));
            }
        }
    }
    Ok(bytes)
}

fn check_bounds(
    pane_id: PaneId,
    point: MousePoint,
    cols: u16,
    rows: u16,
) -> Result<(), ControlResponse> {
    if point.x >= cols || point.y >= rows {
        return Err(ControlResponse::error_with(
            ControlErrorCode::InvalidArgument,
            format!(
                "pane {pane_id}: position ({}, {}) is outside the pane ({cols}x{rows})",
                point.x, point.y
            ),
        ));
    }
    Ok(())
}

fn expand_gesture(action: &MouseAction) -> Vec<MouseEvent> {
    match *action {
        MouseAction::Click {
            position,
            button,
            count,
            modifiers,
        } => {
            let btn = to_lipan_button(button);
            let mods = to_lipan_mods(modifiers);
            let mut events = Vec::with_capacity(usize::from(count) * 2);
            for _ in 0..count {
                events.push(MouseEvent {
                    x: position.x,
                    y: position.y,
                    kind: MouseKind::Down(btn),
                    mods,
                });
                events.push(MouseEvent {
                    x: position.x,
                    y: position.y,
                    kind: MouseKind::Up(btn),
                    mods,
                });
            }
            events
        }
        MouseAction::Scroll {
            position,
            direction,
            count,
            modifiers,
        } => {
            let kind = match direction {
                MouseScrollDirection::Up => MouseKind::ScrollUp,
                MouseScrollDirection::Down => MouseKind::ScrollDown,
            };
            let mods = to_lipan_mods(modifiers);
            (0..count)
                .map(|_| MouseEvent {
                    x: position.x,
                    y: position.y,
                    kind,
                    mods,
                })
                .collect()
        }
        MouseAction::Move {
            position,
            modifiers,
        } => vec![MouseEvent {
            x: position.x,
            y: position.y,
            kind: MouseKind::Moved,
            mods: to_lipan_mods(modifiers),
        }],
        MouseAction::Drag {
            from,
            to,
            button,
            modifiers,
        } => {
            let btn = to_lipan_button(button);
            let mods = to_lipan_mods(modifiers);
            let path = bresenham_drag_path(from, to);
            let mut events = Vec::with_capacity(path.len() + 2);
            events.push(MouseEvent {
                x: from.x,
                y: from.y,
                kind: MouseKind::Down(btn),
                mods,
            });
            for point in path {
                events.push(MouseEvent {
                    x: point.x,
                    y: point.y,
                    kind: MouseKind::Drag(btn),
                    mods,
                });
            }
            events.push(MouseEvent {
                x: to.x,
                y: to.y,
                kind: MouseKind::Up(btn),
                mods,
            });
            events
        }
    }
}

/// Cell path from `from` (exclusive) to `to` (inclusive) using integer Bresenham stepping.
pub(crate) fn bresenham_drag_path(from: MousePoint, to: MousePoint) -> Vec<MousePoint> {
    let mut points = Vec::new();
    let mut x = i32::from(from.x);
    let mut y = i32::from(from.y);
    let x1 = i32::from(to.x);
    let y1 = i32::from(to.y);
    let dx = (x1 - x).abs();
    let dy = -(y1 - y).abs();
    let sx = if x < x1 { 1 } else { -1 };
    let sy = if y < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    while x != x1 || y != y1 {
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        points.push(MousePoint {
            x: x as u16,
            y: y as u16,
        });
    }
    points
}

fn to_lipan_button(button: MouseButton) -> LipanMouseButton {
    match button {
        MouseButton::Left => LipanMouseButton::Left,
        MouseButton::Middle => LipanMouseButton::Middle,
        MouseButton::Right => LipanMouseButton::Right,
    }
}

fn to_lipan_mods(modifiers: MouseModifiers) -> KeyMods {
    KeyMods {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        ..KeyMods::NONE
    }
}

fn event_part_name(kind: MouseKind) -> &'static str {
    match kind {
        MouseKind::Down(_) => "button press",
        MouseKind::Up(_) => "button release",
        MouseKind::Drag(_) => "drags",
        MouseKind::Moved => "pointer motion",
        MouseKind::ScrollUp | MouseKind::ScrollDown => "scroll",
    }
}

fn dec_mode_name(mode: MouseMode) -> &'static str {
    match mode {
        MouseMode::None => "no mouse reporting",
        MouseMode::X10 => "DEC 9 (X10)",
        MouseMode::Normal => "DEC 1000",
        MouseMode::ButtonEvent => "DEC 1002",
        MouseMode::AnyEvent => "DEC 1003",
    }
}

fn report_error_response(
    pane_id: PaneId,
    kind: MouseKind,
    mode: MouseModeState,
    err: MouseReportError,
) -> ControlResponse {
    let part = event_part_name(kind);
    let (code, message) = match err {
        MouseReportError::ReportingDisabled => (
            ControlErrorCode::Unsupported,
            format!("pane {pane_id}: {part}: {err}"),
        ),
        MouseReportError::NotReported => (
            ControlErrorCode::Unsupported,
            format!(
                "pane {pane_id}: {part}: {err}; it enabled {}",
                dec_mode_name(mode.mode)
            ),
        ),
        MouseReportError::OutsideContent | MouseReportError::CoordinateOutOfRange => (
            ControlErrorCode::InvalidArgument,
            format!("pane {pane_id}: {part}: {err}"),
        ),
        MouseReportError::InvalidGeometry => (
            ControlErrorCode::Unavailable,
            format!("pane {pane_id}: {part}: {err}"),
        ),
        _ => (
            ControlErrorCode::RequestFailed,
            format!("pane {pane_id}: {part}: {err}"),
        ),
    };
    ControlResponse::error_with(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::MouseEncoding;

    fn pt(x: u16, y: u16) -> MousePoint {
        MousePoint { x, y }
    }

    #[test]
    fn bresenham_drag_path_excludes_start_includes_end_and_handles_all_octants() {
        assert_eq!(bresenham_drag_path(pt(2, 2), pt(2, 2)), vec![]);
        assert_eq!(
            bresenham_drag_path(pt(0, 0), pt(3, 0)),
            vec![pt(1, 0), pt(2, 0), pt(3, 0)]
        );
        assert_eq!(
            bresenham_drag_path(pt(3, 0), pt(0, 0)),
            vec![pt(2, 0), pt(1, 0), pt(0, 0)]
        );
        assert_eq!(
            bresenham_drag_path(pt(0, 0), pt(2, 2)),
            vec![pt(1, 1), pt(2, 2)]
        );
        assert_eq!(
            bresenham_drag_path(pt(0, 0), pt(3, 1)),
            vec![pt(1, 0), pt(2, 1), pt(3, 1)]
        );
        assert_eq!(
            bresenham_drag_path(pt(1, 4), pt(0, 1)),
            vec![pt(1, 3), pt(0, 2), pt(0, 1)]
        );
    }

    #[test]
    fn x10_click_omits_release_but_drag_is_still_refused() {
        let mode = MouseModeState {
            mode: MouseMode::X10,
            encoding: MouseEncoding::Sgr,
            focus_events_enabled: false,
        };
        let click = MouseAction::Click {
            position: pt(4, 2),
            button: MouseButton::Left,
            count: 2,
            modifiers: MouseModifiers::default(),
        };
        let bytes =
            encode_mouse_action(1, &click, 80, 24, TerminalCellSize::default(), mode).unwrap();
        // SGR press at 1-based (5, 3) repeated twice, with no release ('m').
        assert_eq!(bytes, b"\x1b[<0;5;3M\x1b[<0;5;3M");

        let drag = MouseAction::Drag {
            from: pt(4, 2),
            to: pt(4, 2),
            button: MouseButton::Left,
            modifiers: MouseModifiers::default(),
        };
        let err =
            encode_mouse_action(1, &drag, 80, 24, TerminalCellSize::default(), mode).unwrap_err();
        assert_eq!(err.code, Some(ControlErrorCode::Unsupported));
        assert!(
            err.error.as_deref().unwrap_or("").contains("DEC 9"),
            "{err:?}"
        );
    }
}
