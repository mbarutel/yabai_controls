//! Clockwise / counter-clockwise window order for a space, independent of
//! layout.
//!
//! Windows that share a frame (a stack, or a stacked node inside bsp) form
//! one *slot*. Slots are ordered by the angle of their centre around the
//! centroid of all slot centres; in screen coordinates (y grows downward)
//! ascending `atan2` is clockwise. A slot sitting on the centroid has no
//! angle and goes last. Inside a slot windows follow `stack-index`, so on a
//! pure stack clockwise is `stack.next` with wraparound.

use crate::yabai::model::{Frame, Window, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Clockwise,
    CounterClockwise,
}

/// Slots closer than this to the centroid count as being on it.
const CENTROID_EPSILON: f64 = 1.0;

/// Which windows take part: ordinary app windows that are on screen.
/// Floating windows are included; they are visible and have a position.
pub fn eligible(w: &Window) -> bool {
    w.is_standard() && !w.is_minimized && !w.is_hidden
}

/// The ring, clockwise. Ineligible windows are skipped.
pub fn ring(windows: &[Window]) -> Vec<WindowId> {
    let mut slots: Vec<Slot> = Vec::new();
    for w in windows.iter().filter(|w| eligible(w)) {
        match slots.iter_mut().find(|s| same_frame(&s.frame, &w.frame)) {
            Some(slot) => slot.members.push((w.stack_index, w.id)),
            None => slots.push(Slot {
                frame: w.frame,
                members: vec![(w.stack_index, w.id)],
            }),
        }
    }
    if slots.is_empty() {
        return Vec::new();
    }

    let n = slots.len() as f64;
    let (cx, cy) = slots.iter().fold((0.0, 0.0), |(x, y), s| {
        let (sx, sy) = s.centre();
        (x + sx / n, y + sy / n)
    });

    // (on_centroid, angle, distance): on-centroid slots sort last.
    slots.sort_by(|a, b| {
        let ka = a.key(cx, cy);
        let kb = b.key(cx, cy);
        ka.0.cmp(&kb.0)
            .then(ka.1.total_cmp(&kb.1))
            .then(ka.2.total_cmp(&kb.2))
    });

    slots
        .into_iter()
        .flat_map(|mut s| {
            s.members.sort_unstable();
            s.members.into_iter().map(|(_, id)| id)
        })
        .collect()
}

/// The window to focus when stepping from `current` in `direction`.
/// `None` when the ring is empty or has only one window; the first entry
/// when `current` is not in the ring (nothing focused, or a dialog is).
pub fn neighbour(
    ring: &[WindowId],
    current: Option<WindowId>,
    direction: Direction,
) -> Option<WindowId> {
    let len = ring.len();
    if len < 2 {
        return None;
    }
    let Some(pos) = current.and_then(|c| ring.iter().position(|id| *id == c)) else {
        return ring.first().copied();
    };
    let next = match direction {
        Direction::Clockwise => (pos + 1) % len,
        Direction::CounterClockwise => (pos + len - 1) % len,
    };
    Some(ring[next])
}

struct Slot {
    frame: Frame,
    /// `(stack_index, id)`; sorted before flattening.
    members: Vec<(u32, WindowId)>,
}

impl Slot {
    fn centre(&self) -> (f64, f64) {
        (
            self.frame.x + self.frame.w / 2.0,
            self.frame.y + self.frame.h / 2.0,
        )
    }

    fn key(&self, cx: f64, cy: f64) -> (bool, f64, f64) {
        let (x, y) = self.centre();
        let (dx, dy) = (x - cx, y - cy);
        let distance = dx.hypot(dy);
        (distance < CENTROID_EPSILON, dy.atan2(dx), distance)
    }
}

fn same_frame(a: &Frame, b: &Frame) -> bool {
    // yabai reports identical frames for stacked windows; tolerate rounding.
    (a.x - b.x).abs() < 0.5
        && (a.y - b.y).abs() < 0.5
        && (a.w - b.w).abs() < 0.5
        && (a.h - b.h).abs() < 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: WindowId, x: f64, y: f64, w: f64, h: f64) -> Window {
        Window {
            id,
            role: "AXWindow".into(),
            subrole: "AXStandardWindow".into(),
            frame: Frame { x, y, w, h },
            ..Window::default()
        }
    }

    fn stacked(id: WindowId, stack_index: u32) -> Window {
        Window {
            stack_index,
            ..win(id, 0.0, 0.0, 100.0, 100.0)
        }
    }

    #[test]
    fn two_by_two_grid_is_walked_clockwise_from_top_left() {
        let windows = [
            win(3, 0.0, 50.0, 50.0, 50.0),  // bottom-left
            win(2, 50.0, 50.0, 50.0, 50.0), // bottom-right
            win(1, 50.0, 0.0, 50.0, 50.0),  // top-right
            win(0, 0.0, 0.0, 50.0, 50.0),   // top-left
        ];
        assert_eq!(ring(&windows), vec![0, 1, 2, 3]);
    }

    #[test]
    fn side_by_side_alternates_and_column_goes_down() {
        let row = [
            win(1, 0.0, 0.0, 50.0, 100.0),
            win(2, 50.0, 0.0, 50.0, 100.0),
        ];
        let r = ring(&row);
        assert_eq!(neighbour(&r, Some(1), Direction::Clockwise), Some(2));
        assert_eq!(neighbour(&r, Some(2), Direction::Clockwise), Some(1));

        let column = [
            win(3, 0.0, 66.0, 100.0, 33.0),
            win(1, 0.0, 0.0, 100.0, 33.0),
            win(2, 0.0, 33.0, 100.0, 33.0),
        ];
        // Middle window is on the centroid: goes last, so top → bottom → middle.
        assert_eq!(ring(&column), vec![1, 3, 2]);
    }

    #[test]
    fn bsp_with_a_stacked_node_walks_the_stack_in_place() {
        // Left: one window. Right: a stack of three (identical frames).
        let windows = [
            win(10, 0.0, 0.0, 50.0, 100.0),
            Window {
                stack_index: 2,
                ..win(22, 50.0, 0.0, 50.0, 100.0)
            },
            Window {
                stack_index: 3,
                ..win(23, 50.0, 0.0, 50.0, 100.0)
            },
            Window {
                stack_index: 1,
                ..win(21, 50.0, 0.0, 50.0, 100.0)
            },
        ];
        let r = ring(&windows);
        assert_eq!(r, vec![21, 22, 23, 10]);
        assert_eq!(neighbour(&r, Some(23), Direction::Clockwise), Some(10));
        assert_eq!(
            neighbour(&r, Some(21), Direction::CounterClockwise),
            Some(10)
        );
    }

    #[test]
    fn pure_stack_follows_stack_index_with_wraparound() {
        let windows = [stacked(7, 3), stacked(5, 1), stacked(6, 2)];
        let r = ring(&windows);
        assert_eq!(r, vec![5, 6, 7]);
        assert_eq!(neighbour(&r, Some(7), Direction::Clockwise), Some(5));
        assert_eq!(neighbour(&r, Some(5), Direction::CounterClockwise), Some(7));
    }

    #[test]
    fn centroid_window_goes_last_in_a_three_by_three() {
        let mut windows = Vec::new();
        let mut id = 0;
        for row in 0..3 {
            for col in 0..3 {
                windows.push(win(id, col as f64 * 10.0, row as f64 * 10.0, 10.0, 10.0));
                id += 1;
            }
        }
        let r = ring(&windows);
        assert_eq!(*r.last().unwrap(), 4, "centre window last");
        assert_eq!(&r[..8], &[0, 1, 2, 5, 8, 7, 6, 3], "outer ring clockwise");
    }

    #[test]
    fn floating_windows_are_included_minimized_and_dialogs_are_not() {
        let floating = Window {
            is_floating: true,
            ..win(2, 60.0, 0.0, 40.0, 40.0)
        };
        let minimized = Window {
            is_minimized: true,
            ..win(3, 60.0, 60.0, 40.0, 40.0)
        };
        let dialog = Window {
            subrole: "AXDialog".into(),
            ..win(4, 0.0, 60.0, 40.0, 40.0)
        };
        let windows = [win(1, 0.0, 0.0, 50.0, 100.0), floating, minimized, dialog];
        let mut r = ring(&windows);
        r.sort_unstable();
        assert_eq!(r, vec![1, 2]);
    }

    #[test]
    fn neighbour_handles_degenerate_rings() {
        assert_eq!(neighbour(&[], Some(1), Direction::Clockwise), None);
        assert_eq!(neighbour(&[1], Some(1), Direction::Clockwise), None);
        assert_eq!(
            neighbour(&[1, 2], None, Direction::Clockwise),
            Some(1),
            "nothing focused: start at the first slot"
        );
        assert_eq!(
            neighbour(&[1, 2], Some(9), Direction::CounterClockwise),
            Some(1),
            "focused window not in ring (dialog): start at the first slot"
        );
    }
}
