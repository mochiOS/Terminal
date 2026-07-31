use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::session::TerminalSession;
use viewkit::components::{Rectangle, RectangleColor, Text};
use viewkit::event::{EventContext, EventResult, ViewEvent};
use viewkit::geometry::{Rect, Size};
use viewkit::platform::CursorIcon;
use viewkit::theme::Color;
use viewkit::view::{Constraints, MeasureContext, PaintContext, View};

const POLL_INTERVAL: Duration = Duration::from_millis(16);
const CONTENT_PADDING: f32 = 14.0;
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 20.0;
const APPROXIMATE_GLYPH_WIDTH: f32 = 8.4;
const BACKGROUND: Color = Color::from_rgb_hex(0x17181a);
const FOREGROUND: Color = Color::from_rgb_hex(0xe8eaed);

pub(crate) struct TerminalView {
    session: Rc<RefCell<TerminalSession>>,
    paint_state: RefCell<TerminalPaintState>,
}

#[derive(Default)]
struct TerminalPaintState {
    text: String,
    columns: usize,
    rows: usize,
    initialized: bool,
    render_pending: bool,
}

impl TerminalView {
    pub(crate) fn new(session: Rc<RefCell<TerminalSession>>) -> Self {
        Self {
            session,
            paint_state: RefCell::new(TerminalPaintState::default()),
        }
    }

    fn content_bounds(bounds: Rect) -> Rect {
        Rect::new(
            bounds.origin.x + CONTENT_PADDING,
            bounds.origin.y + CONTENT_PADDING,
            (bounds.size.width - CONTENT_PADDING * 2.0).max(0.0),
            (bounds.size.height - CONTENT_PADDING * 2.0).max(0.0),
        )
    }
}

impl View for TerminalView {
    fn measure(&self, constraints: Constraints, _context: &mut MeasureContext<'_>) -> Size {
        constraints.constrain(constraints.maximum)
    }

    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        Rectangle::new()
            .color(RectangleColor::Custom(BACKGROUND))
            .paint(bounds, context);

        let content = Self::content_bounds(bounds);
        let columns = (content.size.width / APPROXIMATE_GLYPH_WIDTH)
            .floor()
            .max(1.0) as usize;
        let rows = (content.size.height / LINE_HEIGHT).floor().max(1.0) as usize;
        let output_changed = {
            let mut session = self.session.borrow_mut();
            session.poll()
        };

        let mut paint_state = self.paint_state.borrow_mut();
        let dimensions_changed = paint_state.columns != columns || paint_state.rows != rows;
        let draw_text =
            !paint_state.initialized || dimensions_changed || paint_state.render_pending;
        if output_changed || draw_text {
            paint_state.text = self.session.borrow().visible_text(columns, rows);
            paint_state.columns = columns;
            paint_state.rows = rows;
            paint_state.initialized = true;
        }

        if output_changed && !draw_text {
            paint_state.render_pending = true;
            context.request_redraw_in_at(bounds, Instant::now());
        }

        if draw_text {
            context
                .display_list
                .push(viewkit::draw_command::DrawCommand::PushClip { rect: content });
            Text::new(paint_state.text.clone())
                .monospaced()
                .cache_layout(false)
                .font_size(FONT_SIZE)
                .line_height(LINE_HEIGHT)
                .color(FOREGROUND)
                .paint(content, context);
            context
                .display_list
                .push(viewkit::draw_command::DrawCommand::PopClip);
            paint_state.render_pending = false;
        }

        let poll_region = Rect::new(bounds.origin.x, bounds.origin.y, 1.0, 1.0);
        context.request_redraw_in_at(poll_region, Instant::now() + POLL_INTERVAL);
    }

    fn handle_event(
        &self,
        bounds: Rect,
        event: &ViewEvent,
        context: &mut EventContext<'_>,
    ) -> EventResult {
        match event {
            ViewEvent::TextInput { text } => {
                let _ = self.session.borrow_mut().send_text(text);
                EventResult::Consumed
            }
            ViewEvent::Backspace => {
                let _ = self.session.borrow_mut().send_backspace();
                EventResult::Consumed
            }
            ViewEvent::KeyPressed { key, modifiers } => {
                if self.session.borrow_mut().send_key(*key, *modifiers) {
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            ViewEvent::Scroll {
                position, delta_y, ..
            } if bounds.contains(*position) => {
                let rows = if *delta_y > 0.0 { 3 } else { -3 };
                if self.session.borrow_mut().scroll(rows) {
                    self.paint_state.borrow_mut().render_pending = true;
                    context.request_redraw_in(bounds);
                }
                EventResult::Consumed
            }
            ViewEvent::PointerMoved { position } if bounds.contains(*position) => {
                context.set_cursor(CursorIcon::Text);
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}
