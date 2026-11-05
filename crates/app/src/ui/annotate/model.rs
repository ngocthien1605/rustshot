//! The annotation editor's pure model: tools, committed items, in-progress drag / text entry,
//! and the image⇄view coordinate mapping. No Win32 here, so it is fully unit-tested; the window
//! in [`super::win`] only forwards input and paints.
//!
//! All coordinates in the model are **image pixels** (top-left origin).
//!
//! Items come in two layers. *Pixel* items ([`Item::Pixelate`], [`Item::Highlight`]) change the
//! screenshot's pixels themselves ([`apply_pixel_layer`]); the window then draws the *vector*
//! items (outlines, arrows, text) on top. Save and Copy export both layers, so a redaction is
//! part of the file, not an overlay that could be lifted off.
//!
//! Each tool remembers its own [`Style`] (colour from a small fixed [`Color`] palette, and a
//! [`Thickness`]); red at medium thickness is the default, as before styles existed.

use capture_core::{Point, Rect};
use imaging::Rgba;

/// A drag must cover at least this many image pixels on both axes to count as a shape (an
/// arrow: at least this long).
pub const MIN_SIZE: i32 = 6;

/// Pixelate block size in image pixels: large enough that text inside a block is not legible.
pub const PIXELATE_BLOCK: u32 = 12;

/// Highlighter opacity (102/255 = 40 %): the text underneath stays readable.
pub const HIGHLIGHT_ALPHA: u8 = 102;

/// The fixed colour palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Red,
    Yellow,
    Green,
    Blue,
    Black,
    White,
}

impl Color {
    /// Every colour, in toolbar order (shortcut keys `1`–`6`).
    pub const PALETTE: [Color; 6] = [
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Blue,
        Color::Black,
        Color::White,
    ];

    pub fn rgb(self) -> [u8; 3] {
        match self {
            Color::Red => [255, 0, 0],
            Color::Yellow => [255, 230, 0],
            Color::Green => [0, 160, 60],
            Color::Blue => [0, 110, 230],
            Color::Black => [0, 0, 0],
            Color::White => [255, 255, 255],
        }
    }
}

/// Line thickness of outlines, arrows and text-box borders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thickness {
    Thin,
    Medium,
    Thick,
}

impl Thickness {
    pub const ALL: [Thickness; 3] = [Thickness::Thin, Thickness::Medium, Thickness::Thick];

    /// Line width in image pixels. Medium is the 3 px the editor always drew.
    pub fn px(self) -> i32 {
        match self {
            Thickness::Thin => 2,
            Thickness::Medium => 3,
            Thickness::Thick => 6,
        }
    }

    pub fn thinner(self) -> Thickness {
        match self {
            Thickness::Thick => Thickness::Medium,
            Thickness::Medium | Thickness::Thin => Thickness::Thin,
        }
    }

    pub fn thicker(self) -> Thickness {
        match self {
            Thickness::Thin => Thickness::Medium,
            Thickness::Medium | Thickness::Thick => Thickness::Thick,
        }
    }
}

/// How a shape is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub color: Color,
    pub thickness: Thickness,
}

impl Style {
    /// Red, medium: what every shape looked like before there was a palette.
    pub const DEFAULT: Style = Style {
        color: Color::Red,
        thickness: Thickness::Medium,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Click an annotation to select it; drag to move it, `Delete` to remove it.
    Select,
    /// Draw a rectangle outline.
    Rect,
    /// Draw a rectangle, then type text inside it.
    Text,
    /// Drag from the tail to the point of an arrow.
    Arrow,
    /// Drag a translucent highlighter box over the screenshot.
    Highlight,
    /// Drag a rectangle whose pixels are pixelated (irreversibly) in the saved/copied image.
    Pixelate,
    /// Drag a rectangle that is filled solid black in the saved/copied image: the strongest
    /// redaction, for what pixelation could leave recognisable.
    BlackBox,
}

impl Tool {
    pub const ALL: [Tool; 7] = [
        Tool::Select,
        Tool::Rect,
        Tool::Text,
        Tool::Arrow,
        Tool::Highlight,
        Tool::Pixelate,
        Tool::BlackBox,
    ];

    fn index(self) -> usize {
        match self {
            Tool::Select => 0,
            Tool::Rect => 1,
            Tool::Text => 2,
            Tool::Arrow => 3,
            Tool::Highlight => 4,
            Tool::Pixelate => 5,
            Tool::BlackBox => 6,
        }
    }

    /// The style a tool starts with: red, medium — except the highlighter, which is yellow.
    pub fn default_style(self) -> Style {
        match self {
            Tool::Highlight => Style {
                color: Color::Yellow,
                ..Style::DEFAULT
            },
            _ => Style::DEFAULT,
        }
    }

    /// Whether the palette applies (a pixelate area has no colour; a black box is always black).
    /// For Select it depends on the selected item: see [`Editor::color_enabled`].
    pub fn uses_color(self) -> bool {
        !matches!(self, Tool::Select | Tool::Pixelate | Tool::BlackBox)
    }

    /// Whether the thickness applies (highlight, pixelate and black box are filled areas).
    pub fn uses_thickness(self) -> bool {
        matches!(self, Tool::Rect | Tool::Text | Tool::Arrow)
    }
}

/// A committed annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Rect(Rect, Style),
    /// Text in the style's colour inside a border of the style's thickness.
    Text {
        rect: Rect,
        text: String,
        style: Style,
    },
    /// An arrow whose point is at `to`.
    Arrow {
        from: Point,
        to: Point,
        style: Style,
    },
    /// A translucent highlighter box, blended into the pixels by [`apply_pixel_layer`].
    Highlight(Rect, Color),
    /// A redacted area: pixelated in [`PIXELATE_BLOCK`] squares by [`apply_pixel_layer`].
    Pixelate(Rect),
    /// A redacted area filled solid black by [`apply_pixel_layer`].
    BlackBox(Rect),
}

impl Item {
    /// True for items that change the screenshot's pixels (drawn below everything else).
    pub fn is_pixel(&self) -> bool {
        matches!(
            self,
            Item::Highlight(..) | Item::Pixelate(_) | Item::BlackBox(_)
        )
    }

    /// The item's style, for the toolbar while it is selected. A highlight reports its colour
    /// (its line is unused); pixelate and black box have none.
    pub fn style(&self) -> Option<Style> {
        match self {
            Item::Rect(_, style) | Item::Text { style, .. } | Item::Arrow { style, .. } => {
                Some(*style)
            }
            Item::Highlight(_, color) => Some(Style {
                color: *color,
                ..Style::DEFAULT
            }),
            Item::Pixelate(_) | Item::BlackBox(_) => None,
        }
    }

    pub fn uses_color(&self) -> bool {
        self.style().is_some()
    }

    pub fn uses_thickness(&self) -> bool {
        matches!(
            self,
            Item::Rect(..) | Item::Text { .. } | Item::Arrow { .. }
        )
    }

    /// The item with its colour replaced (unchanged when it has none).
    fn recolored(&self, color: Color) -> Item {
        let mut item = self.clone();
        match &mut item {
            Item::Rect(_, style) | Item::Text { style, .. } | Item::Arrow { style, .. } => {
                style.color = color
            }
            Item::Highlight(_, c) => *c = color,
            Item::Pixelate(_) | Item::BlackBox(_) => {}
        }
        item
    }

    /// The item with its line thickness replaced (unchanged when it has none).
    fn rethickened(&self, thickness: Thickness) -> Item {
        let mut item = self.clone();
        if let Item::Rect(_, style) | Item::Text { style, .. } | Item::Arrow { style, .. } =
            &mut item
        {
            style.thickness = thickness;
        }
        item
    }

    /// The box that must stay inside the image when the item is moved: its rectangle, or for an
    /// arrow the box spanned by its two ends.
    pub fn bounds(&self) -> Rect {
        match self {
            Item::Rect(r, _)
            | Item::Text { rect: r, .. }
            | Item::Highlight(r, _)
            | Item::Pixelate(r)
            | Item::BlackBox(r) => *r,
            Item::Arrow { from, to, .. } => Rect::from_points(*from, *to),
        }
    }

    /// The item moved by `(dx, dy)`.
    pub fn translated(&self, dx: i32, dy: i32) -> Item {
        let mv = |r: &Rect| Rect::new(r.x + dx, r.y + dy, r.width, r.height);
        let mp = |p: &Point| Point::new(p.x + dx, p.y + dy);
        match self {
            Item::Rect(r, style) => Item::Rect(mv(r), *style),
            Item::Text { rect, text, style } => Item::Text {
                rect: mv(rect),
                text: text.clone(),
                style: *style,
            },
            Item::Arrow { from, to, style } => Item::Arrow {
                from: mp(from),
                to: mp(to),
                style: *style,
            },
            Item::Highlight(r, color) => Item::Highlight(mv(r), *color),
            Item::Pixelate(r) => Item::Pixelate(mv(r)),
            Item::BlackBox(r) => Item::BlackBox(mv(r)),
        }
    }

    /// Whether a click at `p` picks this item, allowing `slop` image pixels of slack. Filled
    /// items and text boxes are picked anywhere inside; a rectangle outline only near its edge
    /// (so whatever is inside it can still be clicked); an arrow near its shaft or head.
    pub fn hit(&self, p: Point, slop: i32) -> bool {
        let grow =
            |r: &Rect, d: i32| Rect::new(r.x - d, r.y - d, r.width + 2 * d, r.height + 2 * d);
        match self {
            Item::Rect(r, style) => {
                let inner = grow(r, -(style.thickness.px() + slop));
                grow(r, slop).contains(p) && (inner.is_empty() || !inner.contains(p))
            }
            Item::Text { rect: r, .. }
            | Item::Highlight(r, _)
            | Item::Pixelate(r)
            | Item::BlackBox(r) => grow(r, slop).contains(p),
            Item::Arrow { from, to, style } => {
                let width = style.thickness.px();
                let g = arrow_geometry(*from, *to, width);
                let head = dist(g.tip, g.shaft_end);
                dist_to_segment(p, *from, *to) <= f64::from(slop + width / 2 + 1)
                    || dist(p, g.tip) <= head + f64::from(slop)
            }
        }
    }
}

fn dist(a: Point, b: Point) -> f64 {
    f64::from(a.x - b.x).hypot(f64::from(a.y - b.y))
}

/// Distance from `p` to the segment `a`–`b`.
fn dist_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (f64::from(b.x - a.x), f64::from(b.y - a.y));
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return dist(p, a);
    }
    let t = ((f64::from(p.x - a.x) * dx + f64::from(p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (f64::from(a.x) + t * dx, f64::from(a.y) + t * dy);
    (f64::from(p.x) - cx).hypot(f64::from(p.y) - cy)
}

/// A one-key shortcut, available while no text box is being typed into (then keys are text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Tool(Tool),
    Color(Color),
    Thinner,
    Thicker,
}

/// The shortcut for a typed character: `S` `R` `T` `A` `H` `P` `B` pick a tool, `1`–`6` a colour,
/// `[` / `]` step the thickness. Case-insensitive. Characters (not key codes) are matched, so
/// the keys work the same on any keyboard layout.
pub fn shortcut(c: char) -> Option<Shortcut> {
    Some(match c.to_ascii_lowercase() {
        's' => Shortcut::Tool(Tool::Select),
        'r' => Shortcut::Tool(Tool::Rect),
        't' => Shortcut::Tool(Tool::Text),
        'a' => Shortcut::Tool(Tool::Arrow),
        'h' => Shortcut::Tool(Tool::Highlight),
        'p' => Shortcut::Tool(Tool::Pixelate),
        'b' => Shortcut::Tool(Tool::BlackBox),
        '[' => Shortcut::Thinner,
        ']' => Shortcut::Thicker,
        d @ '1'..='6' => {
            let i = d as usize - '1' as usize;
            Shortcut::Color(*Color::PALETTE.get(i)?)
        }
        _ => return None,
    })
}

/// A text box that has been drawn and is currently being typed into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typing {
    pub rect: Rect,
    pub text: String,
    pub style: Style,
}

/// What `Esc` did, so the window knows whether to close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeResult {
    /// Cancelled an in-progress drag, move or text entry, or cleared the selection.
    Cancelled,
    /// Nothing was in progress — the caller should close the editor.
    Nothing,
}

/// A selected item being dragged to a new place.
#[derive(Debug)]
struct Moving {
    index: usize,
    /// The item where the drag started; each step moves this copy, so clamping at an edge
    /// never accumulates.
    original: Item,
    start: Point,
    /// All items before the move, for undo.
    before: Vec<Item>,
}

#[derive(Debug)]
pub struct Editor {
    width: i32,
    height: i32,
    tool: Tool,
    items: Vec<Item>,
    /// Earlier states of `items`, most recent last: every change (draw, move, delete, restyle)
    /// records the state before it.
    undo: Vec<Vec<Item>>,
    /// States taken back by undo, most recent last; cleared by any new change.
    redo: Vec<Vec<Item>>,
    drag: Option<(Point, Point)>,
    typing: Option<Typing>,
    /// Index into `items` of the selected item (Select tool).
    selected: Option<usize>,
    moving: Option<Moving>,
    /// How far (image px) a click may miss an item and still select it.
    slop: i32,
    /// Each tool's style, indexed by [`Tool::index`].
    styles: [Style; Tool::ALL.len()],
}

impl Editor {
    pub fn new(width: i32, height: i32) -> Self {
        Editor {
            width,
            height,
            tool: Tool::Rect,
            items: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            drag: None,
            typing: None,
            selected: None,
            moving: None,
            slop: 4,
            styles: Tool::ALL.map(Tool::default_style),
        }
    }

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

    /// The selected item, if any (only with the Select tool).
    pub fn selected(&self) -> Option<&Item> {
        self.items.get(self.selected?)
    }

    /// Set how many image pixels a click may miss an item by (the window keeps this at a few
    /// screen pixels, whatever the zoom).
    pub fn set_hit_slop(&mut self, slop: i32) {
        self.slop = slop.max(1);
    }

    /// The index of the item a click at `p` would select: the topmost one. Outlines, arrows and
    /// text are drawn above the pixel items, so they are checked first.
    pub fn hit_test(&self, p: Point) -> Option<usize> {
        let top_down = |pixel: bool| {
            self.items
                .iter()
                .enumerate()
                .rev()
                .filter(move |(_, it)| it.is_pixel() == pixel)
        };
        top_down(false)
            .chain(top_down(true))
            .find(|(_, it)| it.hit(p, self.slop))
            .map(|(i, _)| i)
    }

    /// The style the toolbar shows: the selected item's (Select tool), otherwise the current
    /// tool's — what the next shape will look like.
    pub fn style(&self) -> Style {
        if self.tool == Tool::Select {
            if let Some(style) = self.selected().and_then(Item::style) {
                return style;
            }
        }
        self.styles[self.tool.index()]
    }

    /// Whether the colour swatches apply right now.
    pub fn color_enabled(&self) -> bool {
        match self.tool {
            Tool::Select => self.selected().is_some_and(Item::uses_color),
            tool => tool.uses_color(),
        }
    }

    /// Whether the line-thickness buttons apply right now.
    pub fn thickness_enabled(&self) -> bool {
        match self.tool {
            Tool::Select => self.selected().is_some_and(Item::uses_thickness),
            tool => tool.uses_thickness(),
        }
    }

    /// Pick a colour: for the selected item (Select tool; one undo step), otherwise for the
    /// current tool. A text box being typed takes it too, so the colour can be chosen after
    /// drawing the box.
    pub fn set_color(&mut self, color: Color) {
        if self.tool == Tool::Select {
            self.restyle(|item| item.recolored(color));
            return;
        }
        self.styles[self.tool.index()].color = color;
        if let Some(t) = self.typing.as_mut() {
            t.style.color = color;
        }
    }

    /// Pick a thickness: for the selected item (Select tool), otherwise for the current tool and
    /// the text box being typed, if any.
    pub fn set_thickness(&mut self, thickness: Thickness) {
        if self.tool == Tool::Select {
            self.restyle(|item| item.rethickened(thickness));
            return;
        }
        self.styles[self.tool.index()].thickness = thickness;
        if let Some(t) = self.typing.as_mut() {
            t.style.thickness = thickness;
        }
    }

    /// Replace the selected item by `change(item)`, as one undo step (none if nothing changes).
    fn restyle(&mut self, change: impl Fn(&Item) -> Item) {
        if self.moving.is_some() {
            return;
        }
        let Some(i) = self.selected else {
            return;
        };
        let Some(new) = self.items.get(i).map(&change) else {
            return;
        };
        if self.items.get(i) != Some(&new) {
            self.record();
            self.items[i] = new;
        }
    }

    /// The rectangle being rubber-banded right now, if any.
    #[cfg(test)]
    pub fn drag_rect(&self) -> Option<Rect> {
        self.drag.map(|(a, b)| self.normalized(a, b))
    }

    /// What the current drag would commit, to draw it live: the tool's own shape (an arrow, a
    /// highlight, a pixelate area; an outline for the Text tool's box). Shown at any size,
    /// although a drag below [`MIN_SIZE`] is discarded on release. Nothing for Select, which
    /// moves the item itself.
    pub fn preview(&self) -> Option<Item> {
        let (a, b) = self.drag?;
        let r = self.normalized(a, b);
        let style = self.styles[self.tool.index()];
        Some(match self.tool {
            Tool::Select => return None,
            Tool::Rect | Tool::Text => Item::Rect(r, style),
            Tool::Arrow => Item::Arrow {
                from: a,
                to: b,
                style,
            },
            Tool::Highlight => Item::Highlight(r, style.color),
            Tool::Pixelate => Item::Pixelate(r),
            Tool::BlackBox => Item::BlackBox(r),
        })
    }

    /// Switch tool. Any text being typed is committed first (or dropped if empty); a move in
    /// progress is cancelled and the selection cleared.
    pub fn set_tool(&mut self, tool: Tool) {
        self.commit_typing();
        self.cancel_move();
        self.drag = None;
        self.selected = None;
        self.tool = tool;
    }

    pub fn on_down(&mut self, p: Point) {
        // Clicking away from a text box finishes it, as in most editors.
        self.commit_typing();
        let p = self.clamp(p);
        if self.tool == Tool::Select {
            self.selected = self.hit_test(p);
            self.moving = self.selected.and_then(|index| {
                Some(Moving {
                    index,
                    original: self.items.get(index)?.clone(),
                    start: p,
                    before: self.items.clone(),
                })
            });
            return;
        }
        self.drag = Some((p, p));
    }

    pub fn on_move(&mut self, p: Point) {
        let p = self.clamp(p);
        if let Some(m) = &self.moving {
            let moved = self.moved(&m.original, p.x - m.start.x, p.y - m.start.y);
            let index = m.index;
            if let Some(item) = self.items.get_mut(index) {
                *item = moved;
            }
            return;
        }
        if let Some((_, end)) = self.drag.as_mut() {
            *end = p;
        }
    }

    /// `item` moved by up to `(dx, dy)`, stopping where its [`Item::bounds`] would leave the
    /// image.
    fn moved(&self, item: &Item, dx: i32, dy: i32) -> Item {
        let b = item.bounds();
        let (max_x, max_y) = ((self.width - 1).max(0), (self.height - 1).max(0));
        let fit = |d: i32, lo: i32, hi: i32| d.max(lo).min(hi.max(lo));
        let dx = fit(dx, -b.left(), max_x - b.right());
        let dy = fit(dy, -b.top(), max_y - b.bottom());
        item.translated(dx, dy)
    }

    /// The index of the item being dragged by the Select tool, if any.
    pub fn moving_index(&self) -> Option<usize> {
        self.moving.as_ref().map(|m| m.index)
    }

    /// True while the mouse button is down on something being drawn or moved.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some() || self.moving.is_some()
    }

    /// Apply a one-key [`Shortcut`]. Returns false (and does nothing) while typing, when the key
    /// belongs to the text.
    pub fn apply_shortcut(&mut self, sc: Shortcut) -> bool {
        if self.is_typing() {
            return false;
        }
        match sc {
            Shortcut::Tool(t) => self.set_tool(t),
            Shortcut::Color(c) => self.set_color(c),
            Shortcut::Thinner => self.set_thickness(self.style().thickness.thinner()),
            Shortcut::Thicker => self.set_thickness(self.style().thickness.thicker()),
        }
        true
    }

    /// Finish the drag. A big-enough shape is committed (or, for the Text tool, becomes a text
    /// box awaiting typing); a tiny one — an accidental click — is discarded. Boxes need
    /// [`MIN_SIZE`] on both axes, an arrow needs that length (a level arrow has no height). A
    /// move that changed something becomes one undo step.
    pub fn on_up(&mut self, p: Point) {
        self.on_move(p);
        if let Some(m) = self.moving.take() {
            if m.before != self.items {
                self.undo.push(m.before);
                self.redo.clear();
            }
            return;
        }
        let Some((a, b)) = self.drag.take() else {
            return;
        };
        let style = self.styles[self.tool.index()];
        if self.tool == Tool::Arrow {
            if arrow_long_enough(a, b) {
                self.push(Item::Arrow {
                    from: a,
                    to: b,
                    style,
                });
            }
            return;
        }
        let r = self.normalized(a, b);
        if r.width < MIN_SIZE || r.height < MIN_SIZE {
            return;
        }
        match self.tool {
            Tool::Rect => self.push(Item::Rect(r, style)),
            Tool::Highlight => self.push(Item::Highlight(r, style.color)),
            Tool::Pixelate => self.push(Item::Pixelate(r)),
            Tool::BlackBox => self.push(Item::BlackBox(r)),
            Tool::Arrow | Tool::Select => {} // handled above
            Tool::Text => {
                self.typing = Some(Typing {
                    rect: r,
                    text: String::new(),
                    style,
                })
            }
        }
    }

    /// `Delete`: remove the selected item (one undo step). Returns true if one was removed.
    pub fn delete_selected(&mut self) -> bool {
        if self.moving.is_some() {
            return false;
        }
        let Some(i) = self.selected.take() else {
            return false;
        };
        if i >= self.items.len() {
            return false;
        }
        self.record();
        self.items.remove(i);
        true
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

    /// `Esc`: cancel what is in progress (a drag, a move, a text box), else clear the selection;
    /// with nothing of the kind, report [`EscapeResult::Nothing`] so the window can close.
    pub fn on_escape(&mut self) -> EscapeResult {
        if self.cancel_move()
            || self.drag.take().is_some()
            || self.typing.take().is_some()
            || self.selected.take().is_some()
        {
            EscapeResult::Cancelled
        } else {
            EscapeResult::Nothing
        }
    }

    /// Undo: cancel an in-progress shape or move, otherwise go back to the state before the
    /// last change (it can be redone). Returns true when a change was undone.
    pub fn undo(&mut self) -> bool {
        if self.cancel_move() || self.drag.take().is_some() || self.typing.take().is_some() {
            return false;
        }
        match self.undo.pop() {
            Some(before) => {
                self.redo.push(std::mem::replace(&mut self.items, before));
                self.selected = None;
                true
            }
            None => false,
        }
    }

    /// Redo: reapply the change the last undo took back. Does nothing while a shape, move or
    /// text box is in progress (finish or cancel it first), or once a new change has been made
    /// since the undo. Returns true when a change was redone.
    pub fn redo(&mut self) -> bool {
        if !self.idle() {
            return false;
        }
        match self.redo.pop() {
            Some(after) => {
                self.undo.push(std::mem::replace(&mut self.items, after));
                self.selected = None;
                true
            }
            None => false,
        }
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty() && self.idle()
    }

    /// Nothing is being drawn, moved or typed.
    fn idle(&self) -> bool {
        self.drag.is_none() && self.typing.is_none() && self.moving.is_none()
    }

    /// Put a moved item back where the move started. Returns true if a move was in progress.
    fn cancel_move(&mut self) -> bool {
        match self.moving.take() {
            Some(m) => {
                self.items = m.before;
                true
            }
            None => false,
        }
    }

    /// Remember the current items before a change; a new change drops the redo list.
    fn record(&mut self) {
        self.undo.push(self.items.clone());
        self.redo.clear();
    }

    /// Commit a new item (one undo step).
    fn push(&mut self, item: Item) {
        self.record();
        self.items.push(item);
    }

    fn commit_typing(&mut self) {
        if let Some(t) = self.typing.take() {
            if !t.text.trim().is_empty() {
                self.push(Item::Text {
                    rect: t.rect,
                    text: t.text,
                    style: t.style,
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
            Item::BlackBox(r) => imaging::blend_rect_in_place(&mut out, *r, [0, 0, 0], 255),
            Item::Highlight(r, color) => {
                imaging::blend_rect_in_place(&mut out, *r, color.rgb(), HIGHLIGHT_ALPHA)
            }
            Item::Rect(..) | Item::Text { .. } | Item::Arrow { .. } => {}
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
        assert_eq!(
            e.items(),
            &[Item::Rect(Rect::new(20, 10, 60, 50), Style::DEFAULT)]
        );
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
        assert_eq!(
            e.items(),
            &[Item::Rect(Rect::new(0, 0, 199, 99), Style::DEFAULT)]
        );
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
                text: "Hello".into(),
                style: Style::DEFAULT,
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
                text: "line1\nline2".into(),
                style: Style::DEFAULT,
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
            Item::Rect(Rect::new(30, 5, 20, 20), Style::DEFAULT),
            Item::Pixelate(area),
            Item::Text {
                rect: Rect::new(30, 25, 25, 10),
                text: "x".into(),
                style: Style::DEFAULT,
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
            apply_pixel_layer(&src, &[Item::Rect(Rect::new(1, 1, 8, 8), Style::DEFAULT)]),
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
                to: Point::new(20, 10),
                style: Style::DEFAULT,
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
                to: Point::new(199, 0),
                style: Style::DEFAULT,
            }]
        );
    }

    #[test]
    fn highlight_tool_commits_a_normalized_box_with_the_min_size_rule() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Highlight);
        drag(&mut e, (90, 40), (10, 30));
        assert_eq!(
            e.items(),
            &[Item::Highlight(Rect::new(10, 30, 80, 10), Color::Yellow)]
        );
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
        assert_eq!(
            e.items(),
            &[Item::Highlight(Rect::new(10, 10, 50, 20), Color::Yellow)]
        );
        e.undo();
        assert!(e.items().is_empty());
    }

    #[test]
    fn preview_shows_the_current_tools_shape() {
        let mut e = Editor::new(200, 100);
        assert_eq!(e.preview(), None);
        let cases = [
            (
                Tool::Rect,
                Item::Rect(Rect::new(10, 10, 40, 20), Style::DEFAULT),
            ),
            (
                Tool::Text,
                Item::Rect(Rect::new(10, 10, 40, 20), Style::DEFAULT),
            ),
            (
                Tool::Highlight,
                Item::Highlight(Rect::new(10, 10, 40, 20), Color::Yellow),
            ),
            (Tool::Pixelate, Item::Pixelate(Rect::new(10, 10, 40, 20))),
            (
                Tool::Arrow,
                Item::Arrow {
                    from: Point::new(50, 30),
                    to: Point::new(10, 10),
                    style: Style::DEFAULT,
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
        let out = apply_pixel_layer(
            &src,
            &[Item::Highlight(Rect::new(5, 5, 10, 10), Color::Yellow)],
        );
        let a = u32::from(HIGHLIGHT_ALPHA);
        let want = Color::Yellow
            .rgb()
            .map(|c| ((u32::from(c) * a + 127) / 255) as u8);
        assert_eq!(out.get_pixel(5, 5).0[..3], want);
        assert_eq!(out.get_pixel(14, 14).0[..3], want);
        assert_eq!(out.get_pixel(4, 5).0, [0, 0, 0, 0], "outside untouched");
        assert_eq!(out.get_pixel(15, 15).0, [0, 0, 0, 0]);
    }

    #[test]
    fn pixel_items_apply_in_order() {
        let src = detailed(24, 24);
        let all = Rect::new(0, 0, 24, 24);
        let hl_then_px = apply_pixel_layer(
            &src,
            &[Item::Highlight(all, Color::Yellow), Item::Pixelate(all)],
        );
        let px_then_hl = apply_pixel_layer(
            &src,
            &[Item::Pixelate(all), Item::Highlight(all, Color::Yellow)],
        );
        assert_ne!(hl_then_px, px_then_hl);
        // Either way the area is redacted: one 12-px block is one colour.
        for out in [&hl_then_px, &px_then_hl] {
            assert_eq!(out.get_pixel(0, 0), out.get_pixel(11, 11));
        }
    }

    #[test]
    fn defaults_keep_the_old_look_red_and_3px() {
        assert_eq!(Style::DEFAULT.color, Color::Red);
        assert_eq!(Color::Red.rgb(), [255, 0, 0]);
        assert_eq!(
            Style::DEFAULT.thickness.px(),
            3,
            "the editor's original line width"
        );
        let e = Editor::new(200, 100);
        assert_eq!(e.tool(), Tool::Rect);
        assert_eq!(e.style(), Style::DEFAULT);
        for tool in [Tool::Rect, Tool::Text, Tool::Arrow] {
            assert_eq!(tool.default_style(), Style::DEFAULT, "{tool:?}");
        }
        assert_eq!(Tool::Highlight.default_style().color, Color::Yellow);
    }

    #[test]
    fn palette_is_six_distinct_colours() {
        let p = Color::PALETTE;
        assert_eq!(p.len(), 6);
        assert_eq!(p[0], Color::Red, "red first: key 1 and the default");
        for (i, a) in p.iter().enumerate() {
            for b in &p[i + 1..] {
                assert_ne!(a.rgb(), b.rgb());
            }
        }
    }

    #[test]
    fn thickness_steps_and_saturates() {
        let px: Vec<i32> = Thickness::ALL.iter().map(|t| t.px()).collect();
        assert!(px.windows(2).all(|w| w[0] < w[1]), "{px:?}");
        assert_eq!(Thickness::Thin.thinner(), Thickness::Thin);
        assert_eq!(Thickness::Thick.thinner(), Thickness::Medium);
        assert_eq!(Thickness::Medium.thicker(), Thickness::Thick);
        assert_eq!(Thickness::Thick.thicker(), Thickness::Thick);
    }

    #[test]
    fn which_tools_use_colour_and_thickness() {
        // Select has no style of its own: it shows the selected item's (tested separately).
        for tool in Tool::ALL.into_iter().filter(|t| *t != Tool::Select) {
            let filled = matches!(tool, Tool::Highlight | Tool::Pixelate | Tool::BlackBox);
            assert_eq!(tool.uses_thickness(), !filled, "{tool:?}");
            let colourless = matches!(tool, Tool::Pixelate | Tool::BlackBox);
            assert_eq!(tool.uses_color(), !colourless, "{tool:?}");
        }
    }

    #[test]
    fn style_is_per_tool_and_stamped_on_each_item() {
        let mut e = Editor::new(200, 100);
        e.set_color(Color::Blue);
        e.set_thickness(Thickness::Thick);
        let blue_thick = Style {
            color: Color::Blue,
            thickness: Thickness::Thick,
        };
        drag(&mut e, (10, 10), (50, 50));

        e.set_tool(Tool::Arrow);
        assert_eq!(e.style(), Style::DEFAULT, "the arrow kept its own style");
        drag(&mut e, (10, 80), (90, 80));

        e.set_tool(Tool::Rect);
        assert_eq!(
            e.style(),
            blue_thick,
            "the rectangle tool remembers its style"
        );
        e.set_color(Color::Green); // later changes don't recolour what is drawn
        assert_eq!(
            e.items(),
            &[
                Item::Rect(Rect::new(10, 10, 40, 40), blue_thick),
                Item::Arrow {
                    from: Point::new(10, 80),
                    to: Point::new(90, 80),
                    style: Style::DEFAULT,
                },
            ]
        );
    }

    #[test]
    fn highlight_uses_the_chosen_colour() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Highlight);
        e.set_color(Color::Green);
        drag(&mut e, (10, 10), (60, 30));
        assert_eq!(
            e.items(),
            &[Item::Highlight(Rect::new(10, 10, 50, 20), Color::Green)]
        );
    }

    #[test]
    fn colour_and_thickness_apply_to_the_text_box_being_typed() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 10), (110, 50));
        type_str(&mut e, "hi");
        e.set_color(Color::Black);
        e.set_thickness(Thickness::Thin);
        let want = Style {
            color: Color::Black,
            thickness: Thickness::Thin,
        };
        assert_eq!(e.typing().unwrap().style, want);
        e.on_enter();
        assert_eq!(
            e.items(),
            &[Item::Text {
                rect: Rect::new(10, 10, 100, 40),
                text: "hi".into(),
                style: want,
            }]
        );
    }

    #[test]
    fn redo_puts_back_what_undo_took_in_order() {
        let mut e = Editor::new(200, 100);
        drag(&mut e, (10, 10), (50, 50));
        drag(&mut e, (60, 10), (100, 50));
        let both = e.items().to_vec();
        assert!(!e.can_redo());
        assert!(e.undo());
        assert!(e.undo());
        assert!(!e.undo(), "nothing left to undo");
        assert!(e.items().is_empty() && e.can_redo());
        assert!(e.redo());
        assert_eq!(e.items(), &both[..1]);
        assert!(e.redo());
        assert_eq!(e.items(), &both[..]);
        assert!(!e.redo(), "nothing left to redo");
    }

    #[test]
    fn a_new_item_after_undo_drops_the_redo_list() {
        let mut e = Editor::new(200, 100);
        drag(&mut e, (10, 10), (50, 50));
        e.undo();
        drag(&mut e, (60, 10), (100, 50));
        assert!(!e.can_redo());
        assert!(!e.redo());
        assert_eq!(e.items().len(), 1);

        // Committing text counts as a new item too.
        e.undo();
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 60), (90, 90));
        type_str(&mut e, "t");
        e.on_enter();
        assert!(!e.redo());
        assert_eq!(e.items().len(), 1);
    }

    #[test]
    fn redo_waits_for_work_in_progress_and_cancelling_keeps_it() {
        let mut e = Editor::new(200, 100);
        drag(&mut e, (10, 10), (50, 50));
        e.undo();

        e.on_down(Point::new(60, 60));
        e.on_move(Point::new(90, 90));
        assert!(!e.can_redo() && !e.redo(), "no redo mid-drag");
        assert!(
            !e.undo(),
            "undo cancels the drag, it does not touch history"
        );

        e.set_tool(Tool::Text);
        drag(&mut e, (10, 60), (90, 90));
        assert!(!e.redo(), "no redo while a text box is open");
        e.on_escape(); // an empty, cancelled box commits nothing

        assert!(e.redo());
        assert_eq!(
            e.items(),
            &[Item::Rect(Rect::new(10, 10, 40, 40), Style::DEFAULT)]
        );
    }

    #[test]
    fn redo_restores_pixel_items_too() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Pixelate);
        drag(&mut e, (10, 10), (50, 50));
        e.undo();
        assert!(e.items().is_empty());
        e.redo();
        assert_eq!(e.items(), &[Item::Pixelate(Rect::new(10, 10, 40, 40))]);
    }

    #[test]
    fn pixel_items_are_highlight_and_pixelate() {
        let r = Rect::new(0, 0, 9, 9);
        assert!(Item::Pixelate(r).is_pixel());
        assert!(Item::Highlight(r, Color::Yellow).is_pixel());
        assert!(!Item::Rect(r, Style::DEFAULT).is_pixel());
        let arrow = Item::Arrow {
            from: Point::new(0, 0),
            to: Point::new(9, 9),
            style: Style::DEFAULT,
        };
        assert!(!arrow.is_pixel());
    }

    #[test]
    fn shortcut_keys_map_to_tools_colours_and_thickness() {
        assert_eq!(shortcut('r'), Some(Shortcut::Tool(Tool::Rect)));
        assert_eq!(shortcut('T'), Some(Shortcut::Tool(Tool::Text)));
        assert_eq!(shortcut('a'), Some(Shortcut::Tool(Tool::Arrow)));
        assert_eq!(shortcut('h'), Some(Shortcut::Tool(Tool::Highlight)));
        assert_eq!(shortcut('P'), Some(Shortcut::Tool(Tool::Pixelate)));
        for (i, c) in ['1', '2', '3', '4', '5', '6'].into_iter().enumerate() {
            assert_eq!(shortcut(c), Some(Shortcut::Color(Color::PALETTE[i])));
        }
        assert_eq!(shortcut('1'), Some(Shortcut::Color(Color::Red)));
        assert_eq!(shortcut('['), Some(Shortcut::Thinner));
        assert_eq!(shortcut(']'), Some(Shortcut::Thicker));
        for c in ['0', '7', 'x', ' ', '\u{1a}', 'ア'] {
            assert_eq!(shortcut(c), None, "{c:?}");
        }
    }

    #[test]
    fn shortcuts_change_the_editor_but_never_while_typing() {
        let mut e = Editor::new(200, 100);
        assert!(e.apply_shortcut(Shortcut::Tool(Tool::Arrow)));
        assert_eq!(e.tool(), Tool::Arrow);
        assert!(e.apply_shortcut(Shortcut::Color(Color::Blue)));
        assert!(e.apply_shortcut(Shortcut::Thicker));
        assert_eq!(
            e.style(),
            Style {
                color: Color::Blue,
                thickness: Thickness::Thick
            }
        );
        e.apply_shortcut(Shortcut::Thinner);
        e.apply_shortcut(Shortcut::Thinner);
        e.apply_shortcut(Shortcut::Thinner);
        assert_eq!(e.style().thickness, Thickness::Thin);

        e.apply_shortcut(Shortcut::Tool(Tool::Text));
        drag(&mut e, (10, 10), (110, 50));
        assert!(
            !e.apply_shortcut(Shortcut::Tool(Tool::Rect)),
            "keys are text now"
        );
        assert!(!e.apply_shortcut(Shortcut::Color(Color::Green)));
        assert_eq!(e.tool(), Tool::Text);
        assert_eq!(e.typing().unwrap().style, Style::DEFAULT);
    }

    #[test]
    fn is_dragging_tracks_the_mouse_button() {
        let mut e = Editor::new(200, 100);
        assert!(!e.is_dragging());
        e.on_down(Point::new(10, 10));
        assert!(e.is_dragging());
        e.on_up(Point::new(12, 12));
        assert!(!e.is_dragging());
    }

    #[test]
    fn black_box_tool_commits_a_clamped_area_with_the_min_size_rule() {
        let mut e = Editor::new(200, 100);
        assert!(e.apply_shortcut(shortcut('b').unwrap()));
        assert_eq!(e.tool(), Tool::BlackBox);
        drag(&mut e, (150, 90), (-20, 30));
        assert_eq!(e.items(), &[Item::BlackBox(Rect::new(0, 30, 150, 60))]);
        assert!(e.items()[0].is_pixel());
        drag(&mut e, (10, 10), (10 + MIN_SIZE - 1, 80));
        assert_eq!(e.items().len(), 1, "too narrow: an accidental click");
        assert_eq!(
            e.preview(),
            None,
            "nothing is previewed once the mouse is up"
        );
        e.on_down(Point::new(10, 10));
        e.on_move(Point::new(30, 20));
        assert_eq!(e.preview(), Some(Item::BlackBox(Rect::new(10, 10, 20, 10))));
    }

    #[test]
    fn black_box_fills_solid_black_and_hides_everything() {
        let src = detailed(30, 20);
        let area = Rect::new(5, 4, 12, 8);
        let out = apply_pixel_layer(&src, &[Item::BlackBox(area)]);
        for y in 0..20 {
            for x in 0..30 {
                let inside = area.contains(Point::new(x as i32, y as i32));
                let want = if inside {
                    [0, 0, 0, 255]
                } else {
                    src.get_pixel(x, y).0
                };
                assert_eq!(out.get_pixel(x, y).0, want, "({x},{y})");
            }
        }
        // Two different sources give the identical redacted area: nothing is recoverable.
        let other = apply_pixel_layer(
            &Rgba::from_raw(30, 20, [200, 150, 100, 255].repeat(30 * 20)).unwrap(),
            &[Item::BlackBox(area)],
        );
        assert_eq!(out.get_pixel(10, 8), other.get_pixel(10, 8));
    }

    /// An editor with a rectangle outline (10,10)-(60,40) and a black box (100,10)-(140,40).
    fn with_two_items() -> Editor {
        let mut e = Editor::new(200, 100);
        drag(&mut e, (10, 10), (60, 40));
        e.set_tool(Tool::BlackBox);
        drag(&mut e, (100, 10), (140, 40));
        e.set_tool(Tool::Select);
        e
    }

    fn click(e: &mut Editor, x: i32, y: i32) {
        e.on_down(Point::new(x, y));
        e.on_up(Point::new(x, y));
    }

    #[test]
    fn select_picks_by_outline_edge_or_filled_area() {
        let mut e = with_two_items();
        click(&mut e, 11, 25); // on the rectangle's left edge
        assert_eq!(e.selected(), Some(&e.items()[0]));
        click(&mut e, 35, 25); // inside the hollow rectangle: nothing there
        assert_eq!(e.selected(), None);
        click(&mut e, 120, 25); // anywhere inside the black box
        assert_eq!(e.selected(), Some(&e.items()[1]));
        click(&mut e, 180, 90); // empty space clears the selection
        assert_eq!(e.selected(), None);
        assert_eq!(e.items().len(), 2, "selecting changes nothing");
    }

    #[test]
    fn hit_slop_allows_a_near_miss() {
        let mut e = with_two_items();
        e.set_hit_slop(3);
        click(&mut e, 97, 25); // 3 px left of the black box
        assert!(e.selected().is_some());
        click(&mut e, 95, 25); // 5 px: a miss
        assert!(e.selected().is_none());
        e.set_hit_slop(0); // never below 1
        click(&mut e, 99, 25);
        assert!(e.selected().is_some());
    }

    #[test]
    fn the_topmost_item_wins_and_outlines_beat_pixel_items() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Highlight);
        drag(&mut e, (10, 10), (100, 60)); // index 0, a pixel item
        e.set_tool(Tool::Rect);
        drag(&mut e, (40, 20), (80, 50)); // index 1, outline over it
        e.set_tool(Tool::Highlight);
        drag(&mut e, (20, 15), (90, 55)); // index 2, drawn last but under the outline
        e.set_tool(Tool::Select);
        assert_eq!(e.hit_test(Point::new(41, 30)), Some(1), "outline on top");
        assert_eq!(e.hit_test(Point::new(60, 35)), Some(2), "latest highlight");
        assert_eq!(e.hit_test(Point::new(12, 12)), Some(0));
    }

    #[test]
    fn arrows_are_picked_near_the_shaft_or_head() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Arrow);
        drag(&mut e, (20, 50), (120, 50));
        e.set_tool(Tool::Select);
        e.set_hit_slop(2);
        assert_eq!(e.hit_test(Point::new(60, 52)), Some(0), "by the shaft");
        assert_eq!(e.hit_test(Point::new(110, 56)), Some(0), "by a barb");
        assert_eq!(e.hit_test(Point::new(60, 60)), None);
    }

    #[test]
    fn dragging_a_selected_item_moves_it_as_one_undo_step() {
        let mut e = with_two_items();
        e.on_down(Point::new(11, 25));
        assert!(e.is_dragging());
        assert_eq!(e.moving_index(), Some(0));
        e.on_move(Point::new(21, 30));
        e.on_move(Point::new(31, 35)); // live, from the original each time
        e.on_up(Point::new(41, 45));
        assert_eq!(
            e.items()[0],
            Item::Rect(Rect::new(40, 30, 50, 30), Style::DEFAULT)
        );
        assert_eq!(e.preview(), None, "Select draws no rubber band");
        assert_eq!(e.moving_index(), None);
        assert!(e.undo());
        assert_eq!(
            e.items()[0],
            Item::Rect(Rect::new(10, 10, 50, 30), Style::DEFAULT)
        );
        assert!(e.redo());
        assert_eq!(e.items()[0].bounds(), Rect::new(40, 30, 50, 30));
        // The black box drawn before the move is untouched by both.
        assert_eq!(e.items()[1], Item::BlackBox(Rect::new(100, 10, 40, 30)));
    }

    #[test]
    fn a_click_without_moving_records_nothing() {
        let mut e = with_two_items();
        click(&mut e, 120, 25);
        e.undo(); // takes back the black box itself, not a no-op "move"
        assert_eq!(e.items().len(), 1);
    }

    #[test]
    fn moving_stops_at_the_image_edges() {
        let mut e = with_two_items();
        e.on_down(Point::new(120, 25));
        e.on_up(Point::new(400, -300)); // far up-right
        assert_eq!(e.items()[1], Item::BlackBox(Rect::new(159, 0, 40, 30)));
        e.on_down(Point::new(170, 10));
        e.on_up(Point::new(-50, 500)); // far down-left
        assert_eq!(e.items()[1], Item::BlackBox(Rect::new(0, 69, 40, 30)));
    }

    #[test]
    fn arrows_and_text_move_whole() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Arrow);
        drag(&mut e, (20, 50), (120, 50));
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 60), (60, 90));
        type_str(&mut e, "hi");
        e.on_enter();
        e.set_tool(Tool::Select);
        e.on_down(Point::new(60, 50));
        e.on_up(Point::new(70, 40));
        assert_eq!(
            e.items()[0],
            Item::Arrow {
                from: Point::new(30, 40),
                to: Point::new(130, 40),
                style: Style::DEFAULT
            }
        );
        e.on_down(Point::new(30, 75));
        e.on_up(Point::new(130, 65));
        assert_eq!(
            e.items()[1],
            Item::Text {
                rect: Rect::new(110, 50, 50, 30),
                text: "hi".into(),
                style: Style::DEFAULT
            }
        );
    }

    #[test]
    fn delete_removes_the_selected_item_and_undo_restores_it() {
        let mut e = with_two_items();
        assert!(!e.delete_selected(), "nothing selected");
        click(&mut e, 11, 25);
        assert!(e.delete_selected());
        assert_eq!(e.items(), &[Item::BlackBox(Rect::new(100, 10, 40, 30))]);
        assert_eq!(e.selected(), None);
        assert!(!e.delete_selected(), "the selection went with it");
        assert!(e.undo());
        assert_eq!(e.items().len(), 2);
        assert_eq!(
            e.items()[0],
            Item::Rect(Rect::new(10, 10, 50, 30), Style::DEFAULT)
        );
    }

    #[test]
    fn escape_cancels_a_move_then_clears_the_selection_then_closes() {
        let mut e = with_two_items();
        e.on_down(Point::new(120, 25));
        e.on_move(Point::new(150, 50));
        assert_eq!(e.on_escape(), EscapeResult::Cancelled);
        assert_eq!(
            e.items()[1],
            Item::BlackBox(Rect::new(100, 10, 40, 30)),
            "put back"
        );
        assert!(!e.is_dragging());
        assert!(
            e.selected().is_some(),
            "still selected after the cancelled move"
        );
        assert_eq!(e.on_escape(), EscapeResult::Cancelled);
        assert!(e.selected().is_none());
        assert_eq!(e.on_escape(), EscapeResult::Nothing);
    }

    #[test]
    fn undo_mid_move_cancels_the_move_only() {
        let mut e = with_two_items();
        e.on_down(Point::new(120, 25));
        e.on_move(Point::new(150, 50));
        assert!(!e.undo());
        assert_eq!(e.items().len(), 2);
        assert_eq!(e.items()[1], Item::BlackBox(Rect::new(100, 10, 40, 30)));
    }

    #[test]
    fn switching_tool_clears_the_selection_and_cancels_a_move() {
        let mut e = with_two_items();
        e.on_down(Point::new(120, 25));
        e.on_move(Point::new(150, 50));
        e.set_tool(Tool::Rect);
        assert_eq!(e.items()[1], Item::BlackBox(Rect::new(100, 10, 40, 30)));
        e.set_tool(Tool::Select);
        assert!(e.selected().is_none());
    }

    #[test]
    fn colour_and_line_restyle_the_selected_item() {
        let mut e = with_two_items();
        assert!(
            !e.color_enabled() && !e.thickness_enabled(),
            "nothing selected"
        );
        click(&mut e, 11, 25);
        assert!(e.color_enabled() && e.thickness_enabled());
        assert_eq!(
            e.style(),
            Style::DEFAULT,
            "the toolbar shows the item's style"
        );
        e.set_color(Color::Blue);
        e.apply_shortcut(Shortcut::Thicker);
        let restyled = Style {
            color: Color::Blue,
            thickness: Thickness::Thick,
        };
        assert_eq!(
            e.items()[0],
            Item::Rect(Rect::new(10, 10, 50, 30), restyled)
        );
        assert_eq!(e.style(), restyled);
        // Two changes, two undo steps; the Rectangle tool's own style never changed.
        e.undo();
        assert_eq!(
            e.items()[0],
            Item::Rect(
                Rect::new(10, 10, 50, 30),
                Style {
                    color: Color::Blue,
                    ..Style::DEFAULT
                }
            )
        );
        e.set_tool(Tool::Rect);
        assert_eq!(e.style(), Style::DEFAULT);
    }

    #[test]
    fn redaction_items_have_no_colour_to_restyle() {
        let mut e = with_two_items();
        click(&mut e, 120, 25);
        assert!(!e.color_enabled() && !e.thickness_enabled());
        let before = e.items().to_vec();
        e.set_color(Color::Red);
        e.set_thickness(Thickness::Thin);
        assert_eq!(e.items(), &before[..]);
        e.undo(); // so nothing was recorded: this takes back the black box itself
        assert_eq!(e.items().len(), 1);
    }

    #[test]
    fn a_highlight_can_be_recoloured_but_has_no_line() {
        let mut e = Editor::new(200, 100);
        e.set_tool(Tool::Highlight);
        drag(&mut e, (10, 10), (60, 40));
        e.set_tool(Tool::Select);
        click(&mut e, 30, 20);
        assert!(e.color_enabled() && !e.thickness_enabled());
        e.set_color(Color::Green);
        assert_eq!(
            e.items(),
            &[Item::Highlight(Rect::new(10, 10, 50, 30), Color::Green)]
        );
    }

    #[test]
    fn selecting_an_item_while_typing_commits_the_text_first() {
        let mut e = with_two_items();
        e.set_tool(Tool::Text);
        drag(&mut e, (10, 60), (60, 90));
        type_str(&mut e, "x");
        e.set_tool(Tool::Select);
        assert_eq!(e.items().len(), 3, "switching tool committed it");
        click(&mut e, 30, 75);
        assert!(matches!(e.selected(), Some(Item::Text { .. })));
    }

    #[test]
    fn select_shortcut_and_item_geometry_helpers() {
        assert_eq!(shortcut('s'), Some(Shortcut::Tool(Tool::Select)));
        let arrow = Item::Arrow {
            from: Point::new(50, 40),
            to: Point::new(10, 20),
            style: Style::DEFAULT,
        };
        assert_eq!(arrow.bounds(), Rect::new(10, 20, 40, 20));
        assert_eq!(arrow.translated(5, -5).bounds(), Rect::new(15, 15, 40, 20));
        assert_eq!(
            dist_to_segment(Point::new(5, 5), Point::new(0, 0), Point::new(10, 0)),
            5.0
        );
        assert_eq!(
            dist_to_segment(Point::new(-3, 4), Point::new(0, 0), Point::new(10, 0)),
            5.0
        );
        assert_eq!(
            dist_to_segment(Point::new(3, 4), Point::new(0, 0), Point::new(0, 0)),
            5.0
        );
    }
}
