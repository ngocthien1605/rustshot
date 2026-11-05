//! The pinned-image window's pure geometry: which zoom it opens at, the zoom steps the mouse
//! wheel moves through, and where the window goes when zooming so the point under the pointer
//! stays put. No Win32 here, so it is unit-tested; [`super::win`] only applies the results.
//!
//! All lengths are device pixels. A screenshot's pixels are device pixels too, so 100 % shows a
//! pinned capture exactly as large as what was captured.

use capture_core::Rect;

/// Zoom steps in percent, smallest first.
pub const ZOOMS: [u32; 12] = [10, 25, 33, 50, 67, 75, 100, 125, 150, 200, 300, 400];

/// The index of 100 % in [`ZOOMS`].
pub const ACTUAL_SIZE: usize = 6;

/// A pinned window never grows past this many pixels on a side, whatever the zoom.
pub const MAX_SIDE: i32 = 8192;

/// The zoom a new pin opens at: 100 % if the image fits in 80 % of the work area, otherwise the
/// largest smaller step that fits (never below the smallest step).
pub fn initial_zoom(img_w: i32, img_h: i32, work_w: i32, work_h: i32) -> usize {
    let (max_w, max_h) = (i64::from(work_w) * 8 / 10, i64::from(work_h) * 8 / 10);
    (0..=ACTUAL_SIZE)
        .rev()
        .find(|&i| {
            let (w, h) = size_at(img_w, img_h, i);
            i64::from(w) <= max_w && i64::from(h) <= max_h
        })
        .unwrap_or(0)
}

/// The window size for the image at zoom step `zoom` (clamped to the table), at least 1×1 and
/// at most [`MAX_SIDE`] on a side.
pub fn size_at(img_w: i32, img_h: i32, zoom: usize) -> (i32, i32) {
    let pct = i64::from(ZOOMS[zoom.min(ZOOMS.len() - 1)]);
    let scale =
        |v: i32| ((i64::from(v.max(1)) * pct + 50) / 100).clamp(1, i64::from(MAX_SIDE)) as i32;
    (scale(img_w), scale(img_h))
}

/// One wheel notch: the next zoom step in (`up`) or out, stopping at the ends.
pub fn step(zoom: usize, up: bool) -> usize {
    let zoom = zoom.min(ZOOMS.len() - 1);
    if up {
        (zoom + 1).min(ZOOMS.len() - 1)
    } else {
        zoom.saturating_sub(1)
    }
}

/// Where the window goes when its size changes from `old` to `new_w × new_h`: the point
/// `(ax, ay)` (screen px, normally the pointer) stays over the same spot of the image.
pub fn zoom_around(old: Rect, new_w: i32, new_h: i32, ax: i32, ay: i32) -> Rect {
    let keep = |start: i32, old_len: i32, new_len: i32, a: i32| -> i32 {
        let off = i64::from(a - start);
        let scaled = off * i64::from(new_len) / i64::from(old_len.max(1));
        a - scaled as i32
    };
    Rect::new(
        keep(old.x, old.width, new_w, ax),
        keep(old.y, old.height, new_h, ay),
        new_w,
        new_h,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn small_captures_open_at_actual_size() {
        assert_eq!(ZOOMS[ACTUAL_SIZE], 100);
        assert_eq!(initial_zoom(800, 600, 1920, 1040), ACTUAL_SIZE);
        assert_eq!(size_at(800, 600, ACTUAL_SIZE), (800, 600));
    }

    #[test]
    fn large_captures_open_at_the_largest_step_that_fits() {
        // 3840×2160 into 80 % of 1920×1040 (1536×832): 33 % gives 1267×713.
        let z = initial_zoom(3840, 2160, 1920, 1040);
        assert_eq!(ZOOMS[z], 33);
        let (w, h) = size_at(3840, 2160, z);
        assert!(w <= 1536 && h <= 832, "{w}×{h}");
        assert!(
            size_at(3840, 2160, z + 1).1 > 832,
            "the next step would not fit"
        );
        // Absurdly large: the smallest step, not a panic.
        assert_eq!(initial_zoom(100_000, 100_000, 800, 600), 0);
    }

    #[test]
    fn sizes_round_and_stay_within_bounds() {
        assert_eq!(size_at(3, 3, 2), (1, 1), "33 % of 3 rounds to 1");
        assert_eq!(size_at(0, -5, ACTUAL_SIZE), (1, 1));
        assert_eq!(size_at(5000, 10, ZOOMS.len() - 1), (MAX_SIDE, 40));
        assert_eq!(
            size_at(100, 100, 99),
            (400, 400),
            "out-of-table step clamps"
        );
    }

    #[test]
    fn wheel_steps_stop_at_the_ends() {
        assert_eq!(step(ACTUAL_SIZE, true), ACTUAL_SIZE + 1);
        assert_eq!(step(ACTUAL_SIZE, false), ACTUAL_SIZE - 1);
        assert_eq!(step(0, false), 0);
        assert_eq!(step(ZOOMS.len() - 1, true), ZOOMS.len() - 1);
        assert_eq!(step(99, false), ZOOMS.len() - 2);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let old = Rect::new(100, 100, 400, 200);
        // Pointer at the image's centre: the window grows evenly around it.
        assert_eq!(
            zoom_around(old, 800, 400, 300, 200),
            Rect::new(-100, 0, 800, 400)
        );
        // Pointer at the top-left corner: the corner stays.
        assert_eq!(
            zoom_around(old, 200, 100, 100, 100),
            Rect::new(100, 100, 200, 100)
        );
        // A quarter of the way across: still a quarter of the way after zooming.
        let r = zoom_around(old, 200, 100, 200, 150);
        assert_eq!(r, Rect::new(150, 125, 200, 100));
        assert_eq!(((200 - r.x) * 4, (150 - r.y) * 4), (r.width, r.height));
    }
}
