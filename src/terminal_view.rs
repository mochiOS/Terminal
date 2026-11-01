//! Fixed-cell VT rendering. Only the active pane accepts keyboard input.
use crate::{session::TerminalSession, settings::Settings};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use viewkit::platform::PointerButton;
use viewkit::{
    event::{EventContext, EventResult, ViewEvent},
    prelude::*,
    view::{Constraints, MeasureContext, PaintContext},
};
#[derive(Clone, Copy, Default)]
struct Grid {
    bounds: Rect,
    width: f32,
    height: f32,
    cols: u16,
    rows: u16,
}
#[derive(Clone)]
struct Selection {
    start: (u16, u16),
    end: (u16, u16),
    text: String,
}
pub(crate) struct TerminalView {
    pub session: Rc<RefCell<TerminalSession>>,
    settings: Rc<RefCell<Settings>>,
    grid: Cell<Grid>,
    selection: RefCell<Option<Selection>>,
    dragging: Cell<bool>,
    mouse_button: Cell<Option<u8>>,
    last_mouse: Cell<Option<(u16, u16)>>,
    pub focused: Cell<bool>,
    pub highlight: Cell<Option<(u16, u16, u16)>>,
    pub notice: RefCell<Option<String>>,
}
#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Color,
    bg: Color,
    bold: bool,
    underline: bool,
}
impl TerminalView {
    pub fn new(session: Rc<RefCell<TerminalSession>>, settings: Rc<RefCell<Settings>>) -> Self {
        Self {
            session,
            settings,
            grid: Cell::new(Grid::default()),
            selection: RefCell::new(None),
            dragging: Cell::new(false),
            mouse_button: Cell::new(None),
            last_mouse: Cell::new(None),
            focused: Cell::new(false),
            highlight: Cell::new(None),
            notice: RefCell::new(None),
        }
    }
    pub fn selected_text(&self) -> Option<String> {
        self.selection
            .borrow()
            .as_ref()
            .map(|s| s.text.clone())
            .filter(|s| !s.is_empty())
    }
    pub fn copy(&self) {
        if let Some(text) = self.selected_text() {
            if !viewkit::platform::clipboard::set_text(&text) {
                *self.notice.borrow_mut() = Some("Clipboard access is unavailable.".into());
            }
        }
    }
    pub fn clear_selection(&self) {
        self.selection.borrow_mut().take();
        self.highlight.set(None);
    }
    pub fn select_all(&self) {
        let session = self.session.borrow();
        let g = self.grid.get();
        self.selection.replace(Some(Selection {
            start: (0, 0),
            end: (g.rows.saturating_sub(1), g.cols.saturating_sub(1)),
            text: session.buffer.screen().contents(),
        }));
    }
    fn position(&self, p: Point) -> (u16, u16) {
        let g = self.grid.get();
        (
            (((p.y - g.bounds.origin.y) / g.height.max(1.0))
                .floor()
                .max(0.0) as u16)
                .min(g.rows.saturating_sub(1)),
            (((p.x - g.bounds.origin.x) / g.width.max(1.0))
                .floor()
                .max(0.0) as u16)
                .min(g.cols.saturating_sub(1)),
        )
    }
    pub fn reports_mouse(&self, modifiers: KeyModifiers) -> bool {
        !modifiers.shift()
            && self.session.borrow().buffer.screen().mouse_protocol_mode()
                != vt100::MouseProtocolMode::None
    }
    fn mouse_event(&self, bounds: Rect, event: &ViewEvent, modifiers: KeyModifiers) -> bool {
        use vt100::MouseProtocolMode as Mode;
        if !self.reports_mouse(modifiers) {
            self.mouse_button.set(None);
            return false;
        }
        let mode = self.session.borrow().buffer.screen().mouse_protocol_mode();
        let Some(position) = event.position() else {
            return false;
        };
        if !bounds.contains(position) && self.mouse_button.get().is_none() {
            return false;
        }
        let cell = self.position(position);
        let (code, release) = match event {
            ViewEvent::PointerPressed { button, .. } => {
                let code = match button {
                    PointerButton::Primary => 0,
                    PointerButton::Middle => 1,
                    PointerButton::Secondary => 2,
                    _ => return false,
                };
                self.mouse_button.set(Some(code));
                (code, false)
            }
            ViewEvent::PointerReleased { .. } => {
                let code = self.mouse_button.take().unwrap_or(0);
                if mode == Mode::Press {
                    return true;
                }
                (code, true)
            }
            ViewEvent::PointerMoved { .. } => {
                if mode != Mode::AnyMotion
                    && !(mode == Mode::ButtonMotion && self.mouse_button.get().is_some())
                {
                    return false;
                }
                if self.last_mouse.get() == Some(cell) {
                    return true;
                }
                (32 + self.mouse_button.get().unwrap_or(3), false)
            }
            ViewEvent::Scroll { delta_y, .. } if *delta_y != 0.0 => {
                (if *delta_y > 0.0 { 64 } else { 65 }, false)
            }
            _ => return false,
        };
        self.last_mouse.set(Some(cell));
        let encoding = self
            .session
            .borrow()
            .buffer
            .screen()
            .mouse_protocol_encoding();
        let bytes = mouse_bytes(code, cell, release, encoding, modifiers);
        self.session.borrow_mut().send(&bytes);
        true
    }
    fn selected(&self, row: u16, col: u16) -> bool {
        let s = self.selection.borrow();
        if let Some(s) = s.as_ref() {
            let (a, b) = if s.start <= s.end {
                (s.start, s.end)
            } else {
                (s.end, s.start)
            };
            (row, col) >= a && (row, col) <= b
        } else {
            false
        }
    }
}
fn color(value: vt100::Color, default: Color) -> Color {
    match value {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color::rgb(r, g, b),
        vt100::Color::Idx(index) => {
            const BASE: [[u8; 3]; 16] = [
                [0, 0, 0],
                [205, 49, 49],
                [13, 188, 121],
                [229, 229, 16],
                [36, 114, 200],
                [188, 63, 188],
                [17, 168, 205],
                [229, 229, 229],
                [102, 102, 102],
                [241, 76, 76],
                [35, 209, 139],
                [245, 245, 67],
                [59, 142, 234],
                [214, 112, 214],
                [41, 184, 219],
                [255, 255, 255],
            ];
            let [r, g, b] = if index < 16 {
                BASE[index as usize]
            } else if index < 232 {
                let n = index - 16;
                let v = |x: u8| if x == 0 { 0 } else { 55 + x * 40 };
                [v(n / 36), v((n / 6) % 6), v(n % 6)]
            } else {
                let v = 8 + (index - 232) * 10;
                [v, v, v]
            };
            Color::rgb(r, g, b)
        }
    }
}
impl View for TerminalView {
    fn measure(&self, c: Constraints, _: &mut MeasureContext<'_>) -> Size {
        c.constrain(c.maximum)
    }
    fn paint(&self, bounds: Rect, c: &mut PaintContext<'_>) {
        let settings = self.settings.borrow();
        let theme = match settings.palette.as_str() {
            "Dark" => Theme::DARK,
            "Light" => Theme::LIGHT,
            _ => *c.theme,
        };
        let font_size = settings.font_size;
        let line = font_size * 1.4;
        let metrics = Text::styled("M", TextRole::Code)
            .monospaced()
            .font_size(font_size)
            .line_height(line)
            .measure(
                Constraints::loose(Size::new(f32::INFINITY, f32::INFINITY)),
                &mut MeasureContext {
                    theme: c.theme,
                    typography: c.typography,
                    text_measurer: c.text_measurer,
                },
            );
        let inset = c.theme.spacing.small;
        let content = Rect::new(
            bounds.origin.x + inset,
            bounds.origin.y + inset,
            (bounds.size.width - inset * 2.0).max(0.0),
            (bounds.size.height - inset * 2.0).max(0.0),
        );
        let width = metrics.width.max(1.0);
        let height = metrics.height.max(1.0);
        let cols = ((content.size.width / width).floor() as usize).clamp(1, 512);
        let rows = ((content.size.height / height).floor() as usize).clamp(1, 256);
        self.grid.set(Grid {
            bounds: content,
            width,
            height,
            cols: cols as u16,
            rows: rows as u16,
        });
        let mut session = self.session.borrow_mut();
        session.resize(cols, rows);
        session.poll();
        Rectangle::new()
            .color(RectangleColor::Custom(theme.surface.pane_background))
            .paint(bounds, c);
        c.display_list
            .push(viewkit::draw_command::DrawCommand::PushClip { rect: content });
        let screen = session.buffer.screen();
        for row in 0..rows as u16 {
            let mut col = 0;
            while col < cols as u16 {
                let cell = screen.cell(row, col).unwrap();
                if cell.is_wide_continuation() {
                    col += 1;
                    continue;
                }
                let mut fg = color(cell.fgcolor(), theme.colors.text_primary);
                let mut bg = color(cell.bgcolor(), theme.surface.pane_background);
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.dim() {
                    fg = fg.with_alpha(150);
                }
                let found = self
                    .highlight
                    .get()
                    .is_some_and(|(r, a, b)| r == row && col >= a && col <= b);
                if self.selected(row, col) || found {
                    bg = if self.focused.get() {
                        c.theme.list.selected_background
                    } else {
                        c.theme.list.inactive_selected_background
                    };
                    fg = if self.focused.get() {
                        c.theme.list.selected_foreground
                    } else {
                        c.theme.list.inactive_selected_foreground
                    };
                }
                let style = Style {
                    fg,
                    bg,
                    bold: cell.bold(),
                    underline: cell.underline(),
                };
                let start = col;
                let mut text = String::new();
                if cell.is_wide() || !cell.contents().is_ascii() {
                    text.push_str(cell.contents());
                    col += if cell.is_wide() { 2 } else { 1 };
                } else {
                    loop {
                        let cell = screen.cell(row, col).unwrap();
                        let mut next_fg = color(cell.fgcolor(), theme.colors.text_primary);
                        let mut next_bg = color(cell.bgcolor(), theme.surface.pane_background);
                        if cell.inverse() {
                            std::mem::swap(&mut next_fg, &mut next_bg);
                        }
                        if cell.dim() {
                            next_fg = next_fg.with_alpha(150);
                        }
                        if self.selected(row, col)
                            || self
                                .highlight
                                .get()
                                .is_some_and(|(r, a, b)| r == row && col >= a && col <= b)
                        {
                            next_bg = if self.focused.get() {
                                c.theme.list.selected_background
                            } else {
                                c.theme.list.inactive_selected_background
                            };
                            next_fg = if self.focused.get() {
                                c.theme.list.selected_foreground
                            } else {
                                c.theme.list.inactive_selected_foreground
                            };
                        }
                        if cell.is_wide()
                            || !cell.contents().is_ascii()
                            || (Style {
                                fg: next_fg,
                                bg: next_bg,
                                bold: cell.bold(),
                                underline: cell.underline(),
                            }) != style
                        {
                            break;
                        }
                        text.push_str(if cell.contents().is_empty() {
                            " "
                        } else {
                            cell.contents()
                        });
                        col += 1;
                        if col >= cols as u16 {
                            break;
                        }
                    }
                }
                let rect = Rect::new(
                    content.origin.x + start as f32 * width,
                    content.origin.y + row as f32 * height,
                    (col - start) as f32 * width,
                    height,
                );
                if style.bg != theme.surface.pane_background {
                    Rectangle::new()
                        .color(RectangleColor::Custom(style.bg))
                        .paint(rect, c);
                }
                if !text.trim().is_empty() {
                    Text::styled(text, TextRole::Code)
                        .monospaced()
                        .font_size(font_size)
                        .line_height(line)
                        .color(style.fg)
                        .weight(if style.bold { 700 } else { 400 })
                        .cache_layout(false)
                        .paint(rect, c);
                }
                if style.underline {
                    Rectangle::new()
                        .color(RectangleColor::Custom(style.fg))
                        .paint(
                            Rect::new(
                                rect.origin.x,
                                rect.origin.y + height - 1.0,
                                rect.size.width,
                                1.0,
                            ),
                            c,
                        );
                }
            }
        }
        if self.focused.get()
            && c.window_active()
            && screen.scrollback() == 0
            && !screen.hide_cursor()
        {
            let (row, col) = screen.cursor_position();
            if row < rows as u16 && col < cols as u16 {
                let rect = Rect::new(
                    content.origin.x + col as f32 * width,
                    content.origin.y + row as f32 * height,
                    width,
                    height,
                );
                let caret = match settings.cursor.as_str() {
                    "Block" => rect,
                    "Underline" => Rect::new(
                        rect.origin.x,
                        rect.origin.y + height - c.theme.text_field.caret_width,
                        width,
                        c.theme.text_field.caret_width,
                    ),
                    _ => Rect::new(
                        rect.origin.x,
                        rect.origin.y,
                        c.theme.text_field.caret_width,
                        height,
                    ),
                };
                Rectangle::new()
                    .color(RectangleColor::Custom(c.theme.text_field.caret.with_alpha(
                        if settings.cursor == "Block" { 100 } else { 255 },
                    )))
                    .paint(caret, c);
            }
        }
        c.display_list
            .push(viewkit::draw_command::DrawCommand::PopClip);
        let mut node = viewkit::accessibility::AccessibilityNode::new(
            viewkit::accessibility::AccessibilityRole::TextField,
            bounds,
        );
        node.label = Some(format!("Terminal: {}", session.title()));
        node.focusable = true;
        node.focused = self.focused.get();
        c.record_accessibility(node);
    }
    fn handle_event(
        &self,
        bounds: Rect,
        event: &ViewEvent,
        c: &mut EventContext<'_>,
    ) -> EventResult {
        if matches!(event, ViewEvent::FocusChanged { focused: false }) {
            self.mouse_button.set(None);
            self.dragging.set(false);
        }
        if self.mouse_event(bounds, event, c.modifiers()) {
            return EventResult::Consumed;
        }
        match event {
            ViewEvent::PointerPressed {
                position,
                button: PointerButton::Primary,
            } if bounds.contains(*position) => {
                let p = self.position(*position);
                let text = self.session.borrow().buffer.selected_text(p, p);
                self.selection.replace(Some(Selection {
                    start: p,
                    end: p,
                    text,
                }));
                self.dragging.set(true);
                self.highlight.set(None);
                c.request_redraw_in(bounds);
                EventResult::Consumed
            }
            ViewEvent::PointerMoved { position } => {
                if self.dragging.get() {
                    let end = self.position(*position);
                    let start = self.selection.borrow().as_ref().unwrap().start;
                    let text = self.session.borrow().buffer.selected_text(start, end);
                    self.selection.replace(Some(Selection { start, end, text }));
                    c.request_redraw_in(bounds);
                }
                if bounds.contains(*position) {
                    c.set_cursor(CursorIcon::Text);
                }
                EventResult::Ignored
            }
            ViewEvent::PointerReleased {
                button: PointerButton::Primary,
                ..
            } => {
                if self.dragging.replace(false) && self.settings.borrow().copy_on_select {
                    self.copy();
                }
                EventResult::Ignored
            }
            ViewEvent::Scroll {
                position, delta_y, ..
            } if bounds.contains(*position) => {
                self.clear_selection();
                self.session
                    .borrow_mut()
                    .scroll(if *delta_y > 0.0 { 3 } else { -3 });
                c.request_redraw_in(bounds);
                EventResult::Consumed
            }
            ViewEvent::TextInput { text } if self.focused.get() => {
                self.clear_selection();
                self.session.borrow_mut().send_text(text);
                c.request_redraw_in(bounds);
                EventResult::Consumed
            }
            ViewEvent::Backspace if self.focused.get() => {
                self.clear_selection();
                self.session.borrow_mut().send_backspace();
                c.request_redraw_in(bounds);
                EventResult::Consumed
            }
            ViewEvent::KeyPressed { key, modifiers } if self.focused.get() => {
                if modifiers.shift()
                    && !modifiers.control()
                    && !modifiers.alt()
                    && matches!(key, Key::PageUp | Key::PageDown | Key::Home | Key::End)
                {
                    let rows = self.grid.get().rows.max(1) as i32;
                    self.clear_selection();
                    self.session.borrow_mut().scroll(match key {
                        Key::PageUp => rows,
                        Key::PageDown => -rows,
                        Key::Home => i32::MAX,
                        _ => i32::MIN + 1,
                    });
                    c.request_redraw_in(bounds);
                    return EventResult::Consumed;
                }
                if *key == Key::Escape && self.selection.borrow().is_some() {
                    self.clear_selection();
                    c.request_redraw_in(bounds);
                    return EventResult::Consumed;
                }
                let handled = self.session.borrow_mut().send_key(*key, *modifiers);
                if handled {
                    self.clear_selection();
                    c.request_redraw_in(bounds);
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            _ => EventResult::Ignored,
        }
    }
}

fn mouse_bytes(
    code: u8,
    (row, col): (u16, u16),
    release: bool,
    encoding: vt100::MouseProtocolEncoding,
    modifiers: KeyModifiers,
) -> Vec<u8> {
    let code =
        code + if modifiers.alt() { 8 } else { 0 } + if modifiers.control() { 16 } else { 0 };
    if encoding == vt100::MouseProtocolEncoding::Sgr {
        format!(
            "\x1b[<{};{};{}{}",
            code,
            col + 1,
            row + 1,
            if release { 'm' } else { 'M' }
        )
        .into_bytes()
    } else {
        let mut bytes = b"\x1b[M".to_vec();
        let values = [
            if release { 3 } else { code } as u16 + 32,
            col + 33,
            row + 33,
        ];
        for value in values {
            if encoding == vt100::MouseProtocolEncoding::Utf8 {
                let mut buf = [0; 4];
                bytes.extend_from_slice(
                    char::from_u32(value as u32)
                        .unwrap()
                        .encode_utf8(&mut buf)
                        .as_bytes(),
                );
            } else {
                bytes.push(value.min(255) as u8);
            }
        }
        bytes
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sgr_mouse_reports_real_cells_and_release() {
        assert_eq!(
            mouse_bytes(
                0,
                (4, 8),
                false,
                vt100::MouseProtocolEncoding::Sgr,
                KeyModifiers::default()
            ),
            b"\x1b[<0;9;5M"
        );
        assert_eq!(
            mouse_bytes(
                2,
                (4, 8),
                true,
                vt100::MouseProtocolEncoding::Sgr,
                KeyModifiers::default()
            ),
            b"\x1b[<2;9;5m"
        );
    }
}
