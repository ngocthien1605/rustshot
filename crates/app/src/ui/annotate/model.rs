//! The annotation editor's pure model: tools, committed items, in-progress drag / text entry,
//! and the image⇄view coordinate mapping. No Win32 here, so it is fully unit-tested; the window
//! in [`super::win`] only forwards input and paints.
//!
//! All coordinates in the model are **image pixels** (top-left origin).
//!
//! Items come in two layers. *Pixel* items ([`Item::Pixelate`], [`Item::Highlight`]) change the
//! screenshot's pixels themselves ([`apply_pixel_layer`]); the window then draws the *vector*
//! items (outlines, arrows, text) on top. Save and Copy export both layers, so a redaction is part of the file, not an overlay
//! that could be lifted off.

use capture_core::{Point, Rect};
use imaging::Rgba;

/// A drag must cover at least this many image pixels on both axes to count as a shape (an
/// arrow: at least this long).
pub const MIN_SIZE: i32 = 6;

/// Pixelate block size in image pixels: large enough that text inside a block is not legible.
pub const PIXELATE_BLOCK: u32 = 12;

/// Highlighter colour and opacity (102/255 = 40 %): the text underneath stays readable.
pub const HIGHLIGHT_RGB: [u8; 3] = [255, 230, 0];
pub const HIGHLIGHT_ALPHA: u8 = 102;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Draw a red rectangle outline.
    Rect,
    /// Draw a red rectangle, then type text inside it.
    Text,
    /// Drag from the tail to the point of an arrow.
    // Not reachable from the toolbar until the window wires the new tools in.
    #[allow(dead_code)]
    Arrow,
    /// Drag a translucent highlighter box over the screenshot.
    #[allow(dead_code)]
    Highlight,
    /// Drag a rectangle whose pixels are pixelated (irreversibly) in the saved/copied image.
    // Not reachable from the toolbar until the window wires the new tools in.
    #[allow(dead_code)]
    Pixelate,
}

/// A committed annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Rect(Rect),
    Text {
        rect: Rect,
        text: String,
    },
    /// An arrow whose point is at `to`.
    Arrow {
        from: Point,
        to: Point,
    },
    /// A translucent highlighter box, blended into the pixels by [`apply_pixel_layer`].
    Highlight(Rect),
    /// A redacted area: pixelated in [`PIXELATE_BLOCK`] squares by [`apply_pixel_layer`].
    Pixelate(Rect),
}

/// A text box that has been drawn and is currently being typed into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typing {
    pub rect: Rect,
    pub text: String,
}

/// What `Esc` did, so the window knows whether to close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeResult {
    /// Cancelled an in-progress drag or text entry.
    Cancelled,
    /// Nothing was in progress — the caller should close the editor.
    Nothing,
}

#[derive(Debug)]
pub struct Editor {
    width: i32,
    height: i32,
    tool: Tool,
    items: Vec<Item>,
    drag: Option<(Point, Point)>,
    typing: Option<Typing>,
}

impl Editor {
    pub fn new(width: i32, height: i32) -> Self {
        Editor {
            width,
            height,
            tool: Tool::Rect,
            items: Vec::new(),
            drag: None,
            typing: None,
        }
    }

    #[cfg(test)]
    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn typing(&self) -> Option<&Typing> {
        self.typing.as_ref()
    }

    pub fn is_typing(&self) -> bool {
        self.typing.is_some()
    }

    /// The rectangle being rubber-banded right now, if any.
    pub fn drag_rect(&self) -> Option<Rect> {
        self.drag.map(|(a, b)| self.normalized(a, b))
    }

    /// What the current drag would commit, to draw it live: the tool's own shape (an arrow, a
    /// highlight, a pixelate area; an outline for the Text tool's box). Shown at any size,
    /// although a drag below [`MIN_SIZE`] is discarded on release.
    // Used by the window from the wiring commit on.
    #[allow(dead_code)]
    pub fn preview(&self) -> Option<Item> {
        let (a, b) = self.drag?;
        let r = self.normalized(a, b);
        Some(match self.tool {
            Tool::Rect | Tool::Text => Item::Rect(r),
            Tool::Arrow => Item::Arrow { from: a, to: b },
            Tool::Highlight => Item::Highlight(r),
            Tool::Pixelate => Item::Pixelate(r),
        })
    }

    /// Switch tool. Any text being typed is committed first (or dropped if empty).
    pub fn set_tool(&mut self, tool: Tool) {
        self.commit_typing();
        self.drag = None;
        self.tool = tool;
    }

    pub fn on_down(&mut self, p: Point) {
        // Clicking away from a text box finishes it, as in most editors.
        self.commit_typing();
        let p = self.clamp(p);
        self.drag = Some((p, p));
    }

    pub fn on_move(&mut self, p: Point) {
        let p = self.clamp(p);
        if let Some((_, end)) = self.drag.as_mut() {
            *end = p;
        }
    }

    /// Finish the drag. A big-enough shape is committed (or, for the Text tool, becomes a text
    /// box awaiting typing); a tiny one — an accidental click — is discarded. Boxes need
    /// [`MIN_SIZE`] on both axes, an arrow needs that length (a level arrow has no height).
    pub fn on_up(&mut self, p: Point) {
        self.on_move(p);
        let Some((a, b)) = self.drag.take() else {
            return;
        };
        if self.tool == Tool::Arrow {
            if arrow_long_enough(a, b) {
                self.items.push(Item::Arrow { from: a, to: b });
            }
            return;
        }
        let r = self.normalized(a, b);
        if r.width < MIN_SIZE || r.height < MIN_SIZE {
            return;
        }
        match self.tool {
            Tool::Rect => self.items.push(Item::Rect(r)),
            Tool::Highlight => self.items.push(Item::Highlight(r)),
            Tool::Pixelate => self.items.push(Item::Pixelate(r)),
            Tool::Arrow => {} // handled above
            Tool::Text => {
                self.typing = Some(Typing {
                    rect: r,
                    text: String::new(),
                })
            }
        }
    }

    /// A typed character. Control characters are ignored (Enter/Backspace have own methods).
    pub fn on_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        if let Some(t) = self.typing.as_mut() {
            t.text.push(c);
        }
    }

    pub fn on_backspace(&mut self) {
        if let Some(t) = self.typing.as_mut() {
            t.text.pop();
        }
    }

    /// `Shift+Enter` / `Alt+Enter`: start a new line in the text box being typed (does not save).
    pub fn on_newline(&mut self) {
        if let Some(t) = self.typing.as_mut() {
            t.text.push('\n');
        }
    }

    /// `Enter`: save the text box. Returns true if there was a text box being typed.
    pub fn on_enter(&mut self) -> bool {
        let was_typing = self.typing.is_some();
        self.commit_typing();
        was_typing
    }

    pub fn on_escape(&mut self) -> EscapeResult {
        if self.drag.take().is_some() || self.typing.take().is_some() {
            EscapeResult::Cancelled
        } else {
            EscapeResult::Nothing
        }
    }

    /// Undo: cancel an in-progress shape, otherwise remove the last committed one.
    pub fn undo(&mut self) {
        if self.drag.take().is_some() || self.typing.take().is_some() {
            return;
        }
        self.items.pop();
    }

    fn commit_typing(&mut self) {
        if let Some(t) = self.typing.take() {
            if !t.text.trim().is_empty() {
                self.items.push(Item::Text {
                    rect: t.rect,
                    text: t.text,
                });
            }
        }
    }

    fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(0, (self.width - 1).max(0)),
            p.y.clamp(0, (self.height - 1).max(0)),
        )
    }

    fn normalized(&self, a: Point, b: Point) -> Rect {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// The screenshot with the pixel items of `items` applied in order (a pixelate drawn over a
/// highlight pixelates the highlight too, and the other way round). Vector items are ignored:
/// the window draws them on top. Pure, so what Save and Copy export is unit-tested here.
pub fn apply_pixel_layer(img: &Rgba, items: &[Item]) -> Rgba {
    let mut out = img.clone();
    for item in items {
        match item {
            Item::Pixelate(r) => imaging::pixelate_in_place(&mut out, *r, PIXELATE_BLOCK),
            Item::Highlight(r) => {
                imaging::blend_rect_in_place(&mut out, *r, HIGHLIGHT_RGB, HIGHLIGHT_ALPHA)
            }
            Item::Rect(_) | Item::Text { .. } | Item::Arrow { .. } => {}
        }
    }
    out
}

/// An arrow counts once it is at least [`MIN_SIZE`] long, in any direction.
pub fn arrow_long_enough(from: Point, to: Point) -> bool {
    let (dx, dy) = (i64::from(to.x - from.x), i64::from(to.y - from.y));
    dx * dx + dy * dy >= i64::from(MIN_SIZE) * i64::from(MIN_SIZE)
}

/// Where to draw an arrow: a filled triangle `tip`–`left`–`right` and a shaft from the tail to
/// `shaft_end`, the middle of the triangle's base (so a thick shaft never pokes past the point).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowGeom {
    pub tail: Point,
    pub shaft_end: Point,
    pub tip: Point,
    pub left: Point,
    pub right: Point,
}

/// Arrow head length per pixel of line width, and its bounds: at least 14 px so a thin arrow
/// still reads as one, at most half the arrow so a short one keeps a visible shaft.
const HEAD_PER_WIDTH: f64 = 4.5;
const HEAD_MIN: f64 = 14.0;
/// Half the head's base width over its length (≈ 27° each side of the shaft).
const HEAD_SPREAD: f64 = 0.5;

/// The geometry of an arrow from `from` to `to` drawn with a `width`-px line. Pure.
pub fn arrow_geometry(from: Point, to: Point, width: i32) -> ArrowGeom {
    let (dx, dy) = (f64::from(to.x - from.x), f64::from(to.y - from.y));
    let len = dx.hypot(dy);
    if len < 1.0 {
        return ArrowGeom {
            tail: from,
            shaft_end: to,
            tip: to,
            left: to,
            right: to,
        };
    }
    let (ux, uy) = (dx / len, dy / len);
    let head = (HEAD_PER_WIDTH * f64::from(width.max(1)))
        .max(HEAD_MIN)
        .min(len / 2.0);
    let (bx, by) = (f64::from(to.x) - ux * head, f64::from(to.y) - uy * head);
    // The normal (-uy, ux) spreads the barbs to either side of the base point.
    let half = head * HEAD_SPREAD;
    let pt = |x: f64, y: f64| Point::new(x.round() as i32, y.round() as i32);
    ArrowGeom {
        tail: from,
        shaft_end: pt(bx, by),
        tip: to,
        left: pt(bx - uy * half, by + ux * half),
        right: pt(bx + uy * half, by - ux * half),
    }
}

/// What pressing Enter does while typing in a text box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterAction {
    /// Save the text into the box.
    Commit,
    /// Insert a line break and keep typing.
    NewLine,
}

/// Plain `Enter` saves; `Shift+Enter` and `Alt+Enter` insert a new line instead. Pure, so the
/// rule is unit-tested independently of the keyboard.
pub fn enter_action(shift: bool, alt: bool) -> EnterAction {
    if shift || alt {
        EnterAction::NewLine
    } else {
        EnterAction::Commit
    }
}

/// Font height (image px) for a text box: scales with the box but stays readable.
pub fn font_px(rect: Rect) -> i32 {
    (rect.height / 2).clamp(14, 32)
}

/// Maps between image pixels and window (client) pixels: the image is shown at `scale`, with its
/// top-left at `(ox, oy)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub scale: f64,
    pub ox: i32,
    pub oy: i32,
}

impl View {
    /// 1:1, no offset — what the exporter uses.
    pub const IDENTITY: View = View {
        scale: 1.0,
        ox: 0,
        oy: 0,
    };

    /// Fit an `iw × ih` image into the `area` rectangle (never upscaling past 1:1), centred.
    pub fn fit(area: Rect, iw: i32, ih: i32) -> View {
        let iw = iw.max(1);
        let ih = ih.max(1);
        let scale = (area.width as f64 / iw as f64)
            .min(area.height as f64 / ih as f64)
            .clamp(0.01, 1.0);
        let dw = (iw as f64 * scale).round() as i32;
        let dh = (ih as f64 * scale).round() as i32;
        View {
            scale,
            ox: area.x + (area.width - dw) / 2,
            oy: area.y + (area.height - dh) / 2,
        }
    }

    pub fn px(self, v: i32) -> i32 {
        (v as f64 * self.scale).round() as i32
    }

    #[cfg(test)]
    pub fn to_view(self, p: Point) -> Point {
        Point::new(self.ox + self.px(p.x), self.oy + self.px(p.y))
    }

    pub fn to_image(self, x: i32, y: i32) -> Point {
        Point::new(
            ((x - self.ox) as f64 / self.scale).round() as i32,
            ((y - self.oy) as f64 / self.scale).round() as i32,
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn drag(e: &mut Editor, a: (i32, i32), b: (i32, i32)) {
        e.on_down(Point::new(a.0, a.1));
        e.on_move(Point::new((a.0 + b.0) / 2, (a.1 + b.1) / 2));
        e.on_up(Point::new(b.0, b.1));
    }

    fn type_str(e: &mut Editor, s: &str) {
        for c in s.chars() {
            e.on_char(c);
        }
    }

    #[test]
    fn rect_tool_commits_a_normalized_rectangle() {
        let mut e = Editor::new(200, 100);
        drag(&mut e, (80, 60), (20, 10)); // dragged up-left
        assert_eq!(e.items(), &[Item::Rect(Rect::new(20, 10, 60, 50))]);
    }

    #[test]
    fn tiny_drag_is_an_accidental_click_and_is_discarded() {
        let mut e = Editor::new(200, 100);
        drag(&mut e, (50, 50), (53, 52));
        assert!(e.items().is_empty());
        assert!(!e.is_typing());
    }

    #[test]
    fn drag_is_clamped_to_the_image() {
        let mut e = Editor::new(200, 100);
        drag(&mut e, (-30, -30), (500, 500));
        assert_eq!(e.items(), &[Item::Rect(Rect::new(0, 0, 199, 99))]);
    }

    #[test]
    fn drag_rect_is_live_while_dragging() {
        let mut e = Editor::new(200, 100);
        e.on_down(Point::new(10, 10));
        e.on_move(Point::new(60, 40));
        assert_eq!(e.drag_rect(), Some(Rect::new(10, 10, 50, 30)));
        e.on_up(Point::new(60, 40));
        assert_eq!(e.drag_rect(), None);
    }

    #[test]
    fn text_tool_draw_then_type_then_enter_saves() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        assert!(e.is_typing(), "drawing the box starts text entry");
        assert!(e.items().is_empty(), "not saved until Enter");

        type_str(&mut e, "Hello");
        assert_eq!(e.typing().unwrap().text, "Hello");

        assert!(e.on_enter());
        assert!(!e.is_typing());
        assert_eq!(
            e.items(),
            &[Item::Text {
                rect: Rect::new(10, 10, 100, 40),
                text: "Hello".into()
            }]
        );
    }

    #[test]
    fn backspace_edits_and_control_chars_are_ignored() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        type_str(&mut e, "abc");
        e.on_char('\u{8}'); // raw control char must not be inserted
        e.on_char('\r');
        e.on_backspace();
        assert_eq!(e.typing().unwrap().text, "ab");
    }

    #[test]
    fn only_plain_enter_saves_shift_or_alt_enter_makes_a_new_line() {
        assert_eq!(enter_action(false, false), EnterAction::Commit);
        assert_eq!(enter_action(true, false), EnterAction::NewLine);
        assert_eq!(enter_action(false, true), EnterAction::NewLine);
        assert_eq!(enter_action(true, true), EnterAction::NewLine);
    }

    #[test]
    fn newline_keeps_typing_and_does_not_save() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        type_str(&mut e, "line1");
        e.on_newline();
        type_str(&mut e, "line2");
        assert!(e.is_typing(), "a new line must not save the text");
        assert!(e.items().is_empty());
        assert_eq!(e.typing().unwrap().text, "line1\nline2");

        assert!(
            e.on_enter(),
            "plain Enter then saves the whole multi-line text"
        );
        assert_eq!(
            e.items(),
            &[Item::Text {
                rect: Rect::new(10, 10, 100, 40),
                text: "line1\nline2".into()
            }]
        );
    }

    #[test]
    fn newline_without_a_text_box_is_ignored() {
        let mut e = Editor::new(200, 100);
        e.on_newline();
        assert!(!e.is_typing() && e.items().is_empty());
    }

    #[test]
    fn backspace_can_remove_a_newline() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        type_str(&mut e, "a");
        e.on_newline();
        e.on_backspace();
        assert_eq!(e.typing().unwrap().text, "a");
    }

    #[test]
    fn enter_on_an_empty_text_box_discards_it() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        type_str(&mut e, "   ");
        assert!(e.on_enter());
        assert!(e.items().is_empty());
    }

    #[test]
    fn enter_without_a_text_box_does_nothing() {
        let mut e = Editor::new(200, 100);
        assert!(!e.on_enter());
    }

    #[test]
    fn clicking_elsewhere_commits_the_text_being_typed() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        type_str(&mut e, "note");
        e.on_down(Point::new(150, 80));
        assert!(!e.is_typing());
        assert_eq!(e.items().len(), 1);
    }

    #[test]
    fn switching_tool_commits_text() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        type_str(&mut e, "x");
        e.set_tool(Tool::Rect);
        assert_eq!(e.items().len(), 1);
        assert_eq!(e.tool(), Tool::Rect);
    }

    #[test]
    fn escape_cancels_in_progress_work_then_reports_nothing() {
        let mut e = Editor::new(200, 100);
        e.on_down(Point::new(10, 10));
        e.on_move(Point::new(60, 40));
        assert_eq!(e.on_escape(), EscapeResult::Cancelled);
        assert!(e.items().is_empty());

        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        type_str(&mut e, "draft");
        assert_eq!(e.on_escape(), EscapeResult::Cancelled);
        assert!(e.items().is_empty(), "Esc discards the unsaved text");

        assert_eq!(e.on_escape(), EscapeResult::Nothing);
    }

    #[test]
    fn undo_cancels_in_progress_else_pops_last_item() {
        let mut e = Editor::new(200, 100);
        drag(&mut e, (10, 10), (50, 50));
        drag(&mut e, (60, 10), (100, 50));
        assert_eq!(e.items().len(), 2);
        e.undo();
        assert_eq!(e.items().len(), 1);

        e.set_tool(Tool::Text);
        drag(&mut e, (10, 60), (90, 90));
        e.undo(); // cancels the text box, keeps the committed rect
        assert!(!e.is_typing());
        assert_eq!(e.items().len(), 1);
        e.undo();
        assert!(e.items().is_empty());
        e.undo(); // nothing left: no panic
    }

    #[test]
    fn typing_without_a_text_box_is_ignored() {
        let mut e = Editor::new(200, 100);
        type_str(&mut e, "stray");
        assert!(e.items().is_empty() && !e.is_typing());
    }

    #[test]
    fn font_size_scales_with_box_but_is_bounded() {
        assert_eq!(font_px(Rect::new(0, 0, 100, 10)), 14);
        assert_eq!(font_px(Rect::new(0, 0, 100, 40)), 20);
        assert_eq!(font_px(Rect::new(0, 0, 100, 400)), 32);
    }

    #[test]
    fn view_fit_never_upscales_and_centres() {
        // Small image in a big area: stays 1:1, centred.
        let v = View::fit(Rect::new(0, 40, 1000, 600), 200, 100);
        assert_eq!(v.scale, 1.0);
        assert_eq!((v.ox, v.oy), (400, 40 + 250));
        // Big image: scaled down to fit the limiting axis.
        let v = View::fit(Rect::new(0, 0, 500, 500), 1000, 500);
        assert!((v.scale - 0.5).abs() < 1e-9);
        assert_eq!(v.oy, 125);
    }

    #[test]
    fn view_round_trips_points() {
        let v = View::fit(Rect::new(10, 50, 400, 300), 1600, 1200);
        let p = Point::new(800, 600);
        let back = v.to_image(v.to_view(p).x, v.to_view(p).y);
        assert!((back.x - p.x).abs() <= 2 && (back.y - p.y).abs() <= 2);
    }

    #[test]
    fn pixelate_tool_commits_a_normalized_clamped_area() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Pixelate);
        drag(&mut e, (150, 90), (-20, 30));
        assert_eq!(e.items(), &[Item::Pixelate(Rect::new(0, 30, 150, 60))]);
    }

    #[test]
    fn pixelate_tool_follows_the_min_size_rule_and_undo() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Pixelate);
        drag(&mut e, (10, 10), (10 + MIN_SIZE - 1, 80)); // too narrow
        assert!(e.items().is_empty());
        drag(&mut e, (10, 10), (10 + MIN_SIZE, 10 + MIN_SIZE));
        assert_eq!(e.items().len(), 1);
        e.undo();
        assert!(e.items().is_empty());
    }

    /// A `w × h` image in which every pixel differs from its neighbours.
    fn detailed(w: u32, h: u32) -> Rgba {
        let mut raw = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                raw.extend_from_slice(&[
                    (x * 9) as u8,
                    (y * 13) as u8,
                    ((x * 31 + y * 7) % 251) as u8,
                    255,
                ]);
            }
        }
        Rgba::from_raw(w, h, raw).unwrap()
    }

    #[test]
    fn pixel_layer_redacts_pixelate_areas_and_nothing_else() {
        let src = detailed(60, 40);
        let area = Rect::new(0, 0, 24, 24);
        let items = [
            Item::Rect(Rect::new(30, 5, 20, 20)),
            Item::Pixelate(area),
            Item::Text {
                rect: Rect::new(30, 25, 25, 10),
                text: "x".into(),
            },
        ];
        let out = apply_pixel_layer(&src, &items);
        // Each PIXELATE_BLOCK square inside the area is one colour...
        let b = PIXELATE_BLOCK;
        for (bx, by) in [(0, 0), (b, 0), (0, b), (b, b)] {
            let first = out.get_pixel(bx, by);
            for y in by..by + b {
                for x in bx..bx + b {
                    assert_eq!(out.get_pixel(x, y), first, "({x},{y})");
                }
            }
        }
        // ...and everything outside it, including under the vector items, is the source.
        for y in 0..40 {
            for x in 0..60 {
                if x >= 24 || y >= 24 {
                    assert_eq!(out.get_pixel(x, y), src.get_pixel(x, y), "({x},{y})");
                }
            }
        }
    }

    #[test]
    fn pixel_layer_without_pixel_items_is_the_source() {
        let src = detailed(20, 10);
        assert_eq!(apply_pixel_layer(&src, &[]), src);
        assert_eq!(
            apply_pixel_layer(&src, &[Item::Rect(Rect::new(1, 1, 8, 8))]),
            src
        );
    }

    #[test]
    fn arrow_tool_keeps_the_drag_direction_tail_to_point() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Arrow);
        drag(&mut e, (150, 80), (20, 10)); // drawn up-left: the point is where the drag ends
        assert_eq!(
            e.items(),
            &[Item::Arrow {
                from: Point::new(150, 80),
                to: Point::new(20, 10)
            }]
        );
    }

    #[test]
    fn arrow_min_size_is_a_length_so_level_arrows_count() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Arrow);
        drag(&mut e, (10, 50), (10 + MIN_SIZE, 50)); // perfectly level, zero height
        drag(&mut e, (10, 10), (10, 10 + MIN_SIZE)); // perfectly upright
        drag(&mut e, (50, 50), (55, 55)); // 7.07 px diagonal, under MIN_SIZE on each axis
        assert_eq!(e.items().len(), 3);
        drag(&mut e, (80, 80), (84, 84)); // 5.66 px: an accidental click
        drag(&mut e, (90, 90), (90 + MIN_SIZE - 1, 90));
        assert_eq!(e.items().len(), 3);
        assert!(arrow_long_enough(
            Point::new(0, 0),
            Point::new(0, -MIN_SIZE)
        ));
        assert!(!arrow_long_enough(Point::new(3, 3), Point::new(3, 3)));
    }

    #[test]
    fn arrow_is_clamped_to_the_image() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Arrow);
        drag(&mut e, (-50, 50), (400, -9));
        assert_eq!(
            e.items(),
            &[Item::Arrow {
                from: Point::new(0, 50),
                to: Point::new(199, 0)
            }]
        );
    }

    #[test]
    fn highlight_tool_commits_a_normalized_box_with_the_min_size_rule() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Highlight);
        drag(&mut e, (90, 40), (10, 30));
        assert_eq!(e.items(), &[Item::Highlight(Rect::new(10, 30, 80, 10))]);
        drag(&mut e, (10, 60), (150, 60 + MIN_SIZE - 1)); // a long but too-thin stroke
        assert_eq!(e.items().len(), 1);
    }

    #[test]
    fn undo_removes_arrows_and_highlights_and_cancels_their_drags() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Highlight);
        drag(&mut e, (10, 10), (60, 30));
        e.set_tool(Tool::Arrow);
        drag(&mut e, (10, 80), (90, 80));
        e.on_down(Point::new(100, 10));
        e.on_move(Point::new(150, 50));
        e.undo(); // only cancels the arrow being drawn
        assert_eq!(e.items().len(), 2);
        assert!(e.preview().is_none());
        e.undo();
        assert_eq!(e.items(), &[Item::Highlight(Rect::new(10, 10, 50, 20))]);
        e.undo();
        assert!(e.items().is_empty());
    }

    #[test]
    fn preview_shows_the_current_tools_shape() {
        let mut e = Editor::new(200, 100);
        assert_eq!(e.preview(), None);
        let cases = [
            (Tool::Rect, Item::Rect(Rect::new(10, 10, 40, 20))),
            (Tool::Text, Item::Rect(Rect::new(10, 10, 40, 20))),
            (Tool::Highlight, Item::Highlight(Rect::new(10, 10, 40, 20))),
            (Tool::Pixelate, Item::Pixelate(Rect::new(10, 10, 40, 20))),
            (
                Tool::Arrow,
                Item::Arrow {
                    from: Point::new(50, 30),
                    to: Point::new(10, 10),
                },
            ),
        ];
        for (tool, want) in cases {
            e.set_tool(tool);
            e.on_down(Point::new(50, 30));
            e.on_move(Point::new(10, 10));
            assert_eq!(e.preview(), Some(want), "{tool:?}");
            e.on_escape();
        }
    }

    fn dist(a: Point, b: Point) -> f64 {
        f64::from(a.x - b.x).hypot(f64::from(a.y - b.y))
    }

    #[test]
    fn arrow_head_points_at_the_end_and_is_symmetric() {
        let g = arrow_geometry(Point::new(10, 50), Point::new(110, 50), 3);
        assert_eq!((g.tail, g.tip), (Point::new(10, 50), Point::new(110, 50)));
        // Level arrow: base 14 px (the minimum head) behind the tip, barbs 7 px either side.
        assert_eq!(g.shaft_end, Point::new(96, 50));
        assert_eq!((g.left, g.right), (Point::new(96, 57), Point::new(96, 43)));

        // Any direction: the barbs mirror each other about the shaft.
        let g = arrow_geometry(Point::new(0, 0), Point::new(-60, 80), 6);
        assert!((dist(g.tip, g.left) - dist(g.tip, g.right)).abs() <= 1.0);
        let mid = Point::new((g.left.x + g.right.x) / 2, (g.left.y + g.right.y) / 2);
        assert!(
            dist(mid, g.shaft_end) <= 1.0,
            "base centre sits on the shaft"
        );
        assert!(dist(g.tail, g.shaft_end) < dist(g.tail, g.tip));
    }

    #[test]
    fn arrow_head_grows_with_width_but_leaves_a_shaft() {
        let head = |w: i32, len: i32| {
            let g = arrow_geometry(Point::new(0, 0), Point::new(len, 0), w);
            len - g.shaft_end.x
        };
        assert_eq!(head(2, 200), 14, "thin lines get the minimum head");
        assert_eq!(head(6, 200), 27, "4.5 × width");
        assert!(head(8, 200) > head(6, 200));
        assert_eq!(head(6, 20), 10, "never more than half the arrow");
    }

    #[test]
    fn a_zero_length_arrow_has_degenerate_but_finite_geometry() {
        let p = Point::new(7, 9);
        let g = arrow_geometry(p, p, 3);
        assert_eq!((g.shaft_end, g.tip, g.left, g.right), (p, p, p, p));
    }

    #[test]
    fn pixel_layer_blends_highlights_translucently() {
        let src = Rgba::from_raw(20, 20, vec![0; 20 * 20 * 4]).unwrap();
        let out = apply_pixel_layer(&src, &[Item::Highlight(Rect::new(5, 5, 10, 10))]);
        let a = u32::from(HIGHLIGHT_ALPHA);
        let want = HIGHLIGHT_RGB.map(|c| ((u32::from(c) * a + 127) / 255) as u8);
        assert_eq!(out.get_pixel(5, 5).0[..3], want);
        assert_eq!(out.get_pixel(14, 14).0[..3], want);
        assert_eq!(out.get_pixel(4, 5).0, [0, 0, 0, 0], "outside untouched");
        assert_eq!(out.get_pixel(15, 15).0, [0, 0, 0, 0]);
    }

    #[test]
    fn pixel_items_apply_in_order() {
        let src = detailed(24, 24);
        let all = Rect::new(0, 0, 24, 24);
        let hl_then_px = apply_pixel_layer(&src, &[Item::Highlight(all), Item::Pixelate(all)]);
        let px_then_hl = apply_pixel_layer(&src, &[Item::Pixelate(all), Item::Highlight(all)]);
        assert_ne!(hl_then_px, px_then_hl);
        // Either way the area is redacted: one 12-px block is one colour.
        for out in [&hl_then_px, &px_then_hl] {
            assert_eq!(out.get_pixel(0, 0), out.get_pixel(11, 11));
        }
    }
}
