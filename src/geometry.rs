use tui_lipan::prelude::*;

use crate::state::{OFFSCREEN_MIN_VISIBLE, PaneId, ResizeCorner, SplitAxis, TOP_BAR_HEIGHT};

pub fn canvas_bounds_from_viewport(viewport: Rect) -> FloatRect {
    FloatRect {
        x: 0.0,
        y: 0.0,
        w: f32::from(viewport.w),
        h: f32::from(viewport.h.saturating_sub(TOP_BAR_HEIGHT)),
    }
}

pub fn empty_workspace_rect(bounds: FloatRect) -> FloatRect {
    let w = bounds.w.min(46.0).max(bounds.w.min(18.0));
    let h = bounds.h.min(8.0).max(bounds.h.min(4.0));
    FloatRect {
        x: bounds.x + ((bounds.w - w) / 2.0).max(0.0),
        y: bounds.y + ((bounds.h - h) / 2.0).max(0.0),
        w,
        h,
    }
}

pub fn inset_float_rect(rect: FloatRect, inset: f32) -> FloatRect {
    let horizontal = inset * 2.0;
    let vertical = inset * 2.0;
    FloatRect {
        x: rect.x + inset.min(rect.w / 2.0),
        y: rect.y + inset.min(rect.h / 2.0),
        w: (rect.w - horizontal).max(1.0),
        h: (rect.h - vertical).max(1.0),
    }
}

pub fn clamp_window_size(rect: FloatRect, bounds: FloatRect) -> (f32, f32) {
    let max_w = bounds.w.max(1.0);
    let max_h = bounds.h.max(1.0);
    let min_w = max_w.min(18.0);
    let min_h = max_h.min(6.0);
    (
        rect.w.max(1.0).clamp(min_w, max_w),
        rect.h.max(1.0).clamp(min_h, max_h),
    )
}

pub fn clamp_float_rect(rect: FloatRect, bounds: FloatRect) -> FloatRect {
    let (w, h) = clamp_window_size(rect, bounds);
    let max_x = (bounds.x + bounds.w - w).max(bounds.x);
    let max_y = (bounds.y + bounds.h - h).max(bounds.y);
    FloatRect {
        x: rect.x.clamp(bounds.x, max_x),
        y: rect.y.clamp(bounds.y, max_y),
        w,
        h,
    }
}

pub fn clamp_floating_rect(rect: FloatRect, bounds: FloatRect) -> FloatRect {
    let (w, h) = clamp_window_size(rect, bounds);
    let margin_x = OFFSCREEN_MIN_VISIBLE.min(w);
    let margin_y = OFFSCREEN_MIN_VISIBLE.min(h);
    let lo_x = bounds.x + margin_x - w;
    let hi_x = bounds.x + bounds.w - margin_x;
    let lo_y = bounds.y + margin_y - h;
    let hi_y = bounds.y + bounds.h - margin_y;
    FloatRect {
        x: rect.x.clamp(lo_x.min(hi_x), hi_x.max(lo_x)),
        y: rect.y.clamp(lo_y.min(hi_y), hi_y.max(lo_y)),
        w,
        h,
    }
}

pub fn lift_off_float_rect(
    tile_rect: FloatRect,
    remembered: FloatRect,
    bounds: FloatRect,
) -> FloatRect {
    let center_x = tile_rect.x + tile_rect.w / 2.0;
    let center_y = tile_rect.y + tile_rect.h / 2.0;
    clamp_float_rect(
        FloatRect {
            x: center_x - remembered.w / 2.0,
            y: center_y - remembered.h / 2.0,
            w: remembered.w,
            h: remembered.h,
        },
        bounds,
    )
}

pub fn nearest_resize_corner(event: MouseDragEvent) -> ResizeCorner {
    nearest_resize_corner_from_local(
        event.from_local_x,
        event.from_local_y,
        event.target_w,
        event.target_h,
    )
}

pub fn nearest_resize_corner_from_local(
    local_x: u16,
    local_y: u16,
    target_w: u16,
    target_h: u16,
) -> ResizeCorner {
    let x = f32::from(local_x);
    let y = f32::from(local_y);
    let right = f32::from(target_w.saturating_sub(1));
    let bottom = f32::from(target_h.saturating_sub(1));
    let corners = [
        (ResizeCorner::UpperLeft, 0.0, 0.0),
        (ResizeCorner::UpperRight, right, 0.0),
        (ResizeCorner::LowerLeft, 0.0, bottom),
        (ResizeCorner::LowerRight, right, bottom),
    ];

    corners
        .into_iter()
        .min_by(|(_, ax, ay), (_, bx, by)| {
            let a = (x - ax).powi(2) + (y - ay).powi(2);
            let b = (x - bx).powi(2) + (y - by).powi(2);
            a.total_cmp(&b)
        })
        .map(|(corner, _, _)| corner)
        .unwrap_or(ResizeCorner::LowerRight)
}

pub fn resize_float_rect_from_corner(
    rect: FloatRect,
    corner: ResizeCorner,
    dx: f32,
    dy: f32,
    bounds: FloatRect,
) -> FloatRect {
    let rect = clamp_float_rect(rect, bounds);
    let max_w = bounds.w.max(1.0);
    let max_h = bounds.h.max(1.0);
    let min_w = max_w.min(18.0);
    let min_h = max_h.min(6.0);
    let mut left = rect.x;
    let mut right = rect.x + rect.w;
    let mut top = rect.y;
    let mut bottom = rect.y + rect.h;

    match corner {
        ResizeCorner::UpperLeft => {
            left += dx;
            top += dy;
        }
        ResizeCorner::UpperRight => {
            right += dx;
            top += dy;
        }
        ResizeCorner::LowerLeft => {
            left += dx;
            bottom += dy;
        }
        ResizeCorner::LowerRight => {
            right += dx;
            bottom += dy;
        }
    }

    match corner {
        ResizeCorner::UpperLeft | ResizeCorner::LowerLeft => {
            left = left.clamp(bounds.x, right - min_w);
        }
        ResizeCorner::UpperRight | ResizeCorner::LowerRight => {
            right = right.clamp(left + min_w, bounds.x + bounds.w);
        }
    }
    match corner {
        ResizeCorner::UpperLeft | ResizeCorner::UpperRight => {
            top = top.clamp(bounds.y, bottom - min_h);
        }
        ResizeCorner::LowerLeft | ResizeCorner::LowerRight => {
            bottom = bottom.clamp(top + min_h, bounds.y + bounds.h);
        }
    }

    FloatRect {
        x: left,
        y: top,
        w: (right - left).max(1.0),
        h: (bottom - top).max(1.0),
    }
}

/// True when the corner the user grabbed sits on the outer tile-bounds boundary for the
/// given axis. Such an edge has no split divider, so resizing along that axis would move
/// the pane's *inner* divider in an inverted direction — callers skip it.
pub fn grabbed_edge_on_outer_border(
    focused_rect: FloatRect,
    tile_bounds: FloatRect,
    corner: ResizeCorner,
    axis: SplitAxis,
) -> bool {
    const EDGE_EPS: f32 = 0.5;
    let grabbed_left = matches!(corner, ResizeCorner::UpperLeft | ResizeCorner::LowerLeft);
    let grabbed_top = matches!(corner, ResizeCorner::UpperLeft | ResizeCorner::UpperRight);
    match axis {
        SplitAxis::Horizontal if grabbed_left => focused_rect.x <= tile_bounds.x + EDGE_EPS,
        SplitAxis::Horizontal => {
            focused_rect.x + focused_rect.w >= tile_bounds.x + tile_bounds.w - EDGE_EPS
        }
        SplitAxis::Vertical if grabbed_top => focused_rect.y <= tile_bounds.y + EDGE_EPS,
        SplitAxis::Vertical => {
            focused_rect.y + focused_rect.h >= tile_bounds.y + tile_bounds.h - EDGE_EPS
        }
    }
}

pub fn close_rect(rect: FloatRect) -> FloatRect {
    const SCALE: f32 = 0.9;
    let w = (rect.w * SCALE).max(1.0);
    let h = (rect.h * SCALE).max(1.0);
    FloatRect {
        x: rect.x + (rect.w - w) / 2.0,
        y: rect.y + (rect.h - h) / 2.0,
        w,
        h,
    }
}

pub fn default_floating_rect(bounds: FloatRect, seed: u32) -> FloatRect {
    let w = (bounds.w * 0.42).clamp(bounds.w.min(24.0), bounds.w.max(1.0));
    let h = (bounds.h * 0.42).clamp(bounds.h.min(8.0), bounds.h.max(1.0));
    let offset = (seed % 7) as f32 * 3.0;
    clamp_float_rect(
        FloatRect {
            x: bounds.x + 3.0 + offset,
            y: bounds.y + 2.0 + offset / 2.0,
            w,
            h,
        },
        bounds,
    )
}

pub fn tiled_drag_preview_rect(
    tile_rect: FloatRect,
    remembered_float_rect: FloatRect,
    bounds: FloatRect,
    from_local_x: u16,
    from_local_y: u16,
    target_w: u16,
    target_h: u16,
) -> FloatRect {
    let remembered = clamp_floating_rect(remembered_float_rect, bounds);
    let anchor_x = if target_w == 0 {
        0.5
    } else {
        (f32::from(from_local_x) / f32::from(target_w)).clamp(0.0, 1.0)
    };
    let anchor_y = if target_h == 0 {
        0.5
    } else {
        (f32::from(from_local_y) / f32::from(target_h)).clamp(0.0, 1.0)
    };

    clamp_floating_rect(
        FloatRect {
            x: tile_rect.x + f32::from(from_local_x) - remembered.w * anchor_x,
            y: tile_rect.y + f32::from(from_local_y) - remembered.h * anchor_y,
            w: remembered.w,
            h: remembered.h,
        },
        bounds,
    )
}

pub fn canvas_local_point_from_mouse(x: u16, y: u16, bounds: FloatRect) -> (f32, f32) {
    (
        f32::from(x).clamp(bounds.x, bounds.x + bounds.w),
        f32::from(y.saturating_sub(TOP_BAR_HEIGHT)).clamp(bounds.y, bounds.y + bounds.h),
    )
}

pub fn float_rect_contains_point(rect: FloatRect, point: (f32, f32)) -> bool {
    point.0 >= rect.x && point.0 < rect.x + rect.w && point.1 >= rect.y && point.1 < rect.y + rect.h
}

pub fn rect_center(rect: FloatRect) -> (f32, f32) {
    (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
}

pub fn directional_score(
    current: FloatRect,
    candidate: FloatRect,
    direction: crate::state::Direction,
) -> Option<f32> {
    let current_center = rect_center(current);
    let candidate_center = rect_center(candidate);
    let current_right = current.x + current.w;
    let current_bottom = current.y + current.h;
    let candidate_right = candidate.x + candidate.w;
    let candidate_bottom = candidate.y + candidate.h;

    let (primary_gap, cross_overlap, cross_gap, center_offset) = match direction {
        crate::state::Direction::Left => {
            if candidate_center.0 >= current_center.0 && candidate_right > current.x {
                return None;
            }
            (
                (current.x - candidate_right).max(0.0),
                interval_overlap(current.y, current_bottom, candidate.y, candidate_bottom),
                interval_gap(current.y, current_bottom, candidate.y, candidate_bottom),
                (candidate_center.1 - current_center.1).abs(),
            )
        }
        crate::state::Direction::Right => {
            if candidate_center.0 <= current_center.0 && candidate.x < current_right {
                return None;
            }
            (
                (candidate.x - current_right).max(0.0),
                interval_overlap(current.y, current_bottom, candidate.y, candidate_bottom),
                interval_gap(current.y, current_bottom, candidate.y, candidate_bottom),
                (candidate_center.1 - current_center.1).abs(),
            )
        }
        crate::state::Direction::Up => {
            if candidate_center.1 >= current_center.1 && candidate_bottom > current.y {
                return None;
            }
            (
                (current.y - candidate_bottom).max(0.0),
                interval_overlap(current.x, current_right, candidate.x, candidate_right),
                interval_gap(current.x, current_right, candidate.x, candidate_right),
                (candidate_center.0 - current_center.0).abs(),
            )
        }
        crate::state::Direction::Down => {
            if candidate_center.1 <= current_center.1 && candidate.y < current_bottom {
                return None;
            }
            (
                (candidate.y - current_bottom).max(0.0),
                interval_overlap(current.x, current_right, candidate.x, candidate_right),
                interval_gap(current.x, current_right, candidate.x, candidate_right),
                (candidate_center.0 - current_center.0).abs(),
            )
        }
    };

    let overlap_penalty = if cross_overlap > 0.0 {
        0.0
    } else {
        10_000.0 + cross_gap * 100.0
    };

    Some(overlap_penalty + primary_gap * 10.0 + center_offset)
}

pub fn closest_pane_to_rect(
    reference: FloatRect,
    candidates: &[(PaneId, FloatRect)],
) -> Option<PaneId> {
    if candidates.is_empty() {
        return None;
    }

    candidates
        .iter()
        .min_by(|(_, a), (_, b)| {
            rect_proximity(reference, *a).total_cmp(&rect_proximity(reference, *b))
        })
        .map(|(id, _)| *id)
}

fn rect_proximity(reference: FloatRect, candidate: FloatRect) -> f32 {
    edge_distance(reference, candidate) * 1_000_000.0 + center_distance_sq(reference, candidate)
}

fn edge_distance(a: FloatRect, b: FloatRect) -> f32 {
    let dx = if a.x + a.w <= b.x {
        b.x - (a.x + a.w)
    } else if b.x + b.w <= a.x {
        a.x - (b.x + b.w)
    } else {
        0.0
    };
    let dy = if a.y + a.h <= b.y {
        b.y - (a.y + a.h)
    } else if b.y + b.h <= a.y {
        a.y - (b.y + b.h)
    } else {
        0.0
    };

    if dx == 0.0 && dy == 0.0 {
        0.0
    } else if dx == 0.0 {
        dy
    } else if dy == 0.0 {
        dx
    } else {
        (dx * dx + dy * dy).sqrt()
    }
}

fn center_distance_sq(a: FloatRect, b: FloatRect) -> f32 {
    let (ax, ay) = rect_center(a);
    let (bx, by) = rect_center(b);
    let dx = ax - bx;
    let dy = ay - by;
    dx * dx + dy * dy
}

fn interval_overlap(a_start: f32, a_end: f32, b_start: f32, b_end: f32) -> f32 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0.0)
}

fn interval_gap(a_start: f32, a_end: f32, b_start: f32, b_end: f32) -> f32 {
    if a_end < b_start {
        b_start - a_end
    } else if b_end < a_start {
        a_start - b_end
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_floating_rect_keeps_visible_margin() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 30.0,
        };
        let rect = clamp_floating_rect(
            FloatRect {
                x: -100.0,
                y: -100.0,
                w: 20.0,
                h: 10.0,
            },
            bounds,
        );
        assert_eq!(rect.x, OFFSCREEN_MIN_VISIBLE - rect.w);
        assert_eq!(rect.y, OFFSCREEN_MIN_VISIBLE - rect.h);
    }

    #[test]
    fn tiled_drag_preview_allows_offscreen_clipping() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 30.0,
        };
        let tile = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 50.0,
            h: 20.0,
        };
        let remembered = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 10.0,
        };

        let rect = tiled_drag_preview_rect(tile, remembered, bounds, 49, 0, 50, 20);

        assert!(rect.x < bounds.x);
        assert!(rect.x >= OFFSCREEN_MIN_VISIBLE - rect.w);
    }

    #[test]
    fn lift_off_centers_remembered_size_on_tile() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 120.0,
            h: 40.0,
        };
        let tile = FloatRect {
            x: 20.0,
            y: 10.0,
            w: 60.0,
            h: 20.0,
        };
        let remembered = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 10.0,
        };
        let lifted = lift_off_float_rect(tile, remembered, bounds);
        assert_eq!(rect_center(lifted), rect_center(tile));
    }

    #[test]
    fn closest_pane_prefers_adjacent_over_far_pane() {
        let closing = FloatRect {
            x: 60.0,
            y: 20.0,
            w: 40.0,
            h: 20.0,
        };
        let far = (
            1,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
        );
        let adjacent = (
            2,
            FloatRect {
                x: 60.0,
                y: 0.0,
                w: 40.0,
                h: 19.0,
            },
        );
        assert_eq!(
            closest_pane_to_rect(closing, &[far, adjacent]),
            Some(2),
            "adjacent pane should win over a distant top-left pane"
        );
    }

    #[test]
    fn resize_corner_uses_nearest_corner() {
        assert_eq!(
            nearest_resize_corner_from_local(0, 0, 20, 10),
            ResizeCorner::UpperLeft
        );
        assert_eq!(
            nearest_resize_corner_from_local(19, 9, 20, 10),
            ResizeCorner::LowerRight
        );
    }

    #[test]
    fn grabbed_edge_on_outer_border_detects_terminal_edges() {
        let tile_bounds = FloatRect {
            x: 1.0,
            y: 1.0,
            w: 98.0,
            h: 38.0,
        };
        // Left-column tile flush to the left/top/bottom borders; its right edge is the
        // inner divider against the neighbouring column.
        let left_tile = FloatRect {
            x: 1.0,
            y: 1.0,
            w: 48.0,
            h: 38.0,
        };

        // Lower-left corner sits on the terminal border on both axes -> both blocked.
        assert!(grabbed_edge_on_outer_border(
            left_tile,
            tile_bounds,
            ResizeCorner::LowerLeft,
            SplitAxis::Horizontal
        ));
        assert!(grabbed_edge_on_outer_border(
            left_tile,
            tile_bounds,
            ResizeCorner::LowerLeft,
            SplitAxis::Vertical
        ));

        // Lower-right corner: the right edge is the inner divider -> horizontal allowed,
        // while the bottom edge is still the terminal border -> vertical blocked.
        assert!(!grabbed_edge_on_outer_border(
            left_tile,
            tile_bounds,
            ResizeCorner::LowerRight,
            SplitAxis::Horizontal
        ));
        assert!(grabbed_edge_on_outer_border(
            left_tile,
            tile_bounds,
            ResizeCorner::LowerRight,
            SplitAxis::Vertical
        ));
    }
}
