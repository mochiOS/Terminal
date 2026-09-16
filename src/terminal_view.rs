use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::session::TerminalSession;
use viewkit::components::{Rectangle, RectangleColor, Text};
use viewkit::event::{EventContext, EventResult, ViewEvent};
use viewkit::geometry::{Rect, Size};
use viewkit::platform::CursorIcon;
use viewkit::typography::TextRole;
use viewkit::view::{Constraints, MeasureContext, PaintContext, View};

const POLL_INTERVAL: Duration = Duration::from_millis(16);
const APPROXIMATE_GLYPH_WIDTH_FACTOR: f32 = 0.6;

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
}

impl TerminalView {
    pub(crate) fn new(session: Rc<RefCell<TerminalSession>>) -> Self {
        Self {
            session,
            paint_state: RefCell::new(TerminalPaintState::default()),
        }
    }

    fn content_bounds(bounds: Rect, padding: f32) -> Rect {
        Rect::new(
            bounds.origin.x + padding,
            bounds.origin.y + padding,
            (bounds.size.width - padding * 2.0).max(0.0),
            (bounds.size.height - padding * 2.0).max(0.0),
        )
    }
}

impl View for TerminalView {
    fn measure(&self, constraints: Constraints, _context: &mut MeasureContext<'_>) -> Size {
        constraints.constrain(constraints.maximum)
    }

    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        Rectangle::new()
            .color(RectangleColor::Custom(context.theme.colors.background))
            .paint(bounds, context);

        let content = Self::content_bounds(bounds, context.theme.spacing.medium);
        let code_style = context.typography.style(TextRole::Code);
        let font_scale = context.text_measurer.font_scale();
        let font_size = code_style.size * font_scale;
        let line_height = code_style.line_height * font_scale;
        let approximate_glyph_width = font_size * APPROXIMATE_GLYPH_WIDTH_FACTOR;
        let columns = (content.size.width / approximate_glyph_width)
            .floor()
            .max(1.0) as usize;
        let rows = (content.size.height / line_height).floor().max(1.0) as usize;
        let output_changed = {
            let mut session = self.session.borrow_mut();
            session.poll()
        };

        let mut paint_state = self.paint_state.borrow_mut();
        let dimensions_changed = paint_state.columns != columns || paint_state.rows != rows;
        if output_changed || !paint_state.initialized || dimensions_changed {
            paint_state.text = self.session.borrow().visible_text(columns, rows);
            paint_state.columns = columns;
            paint_state.rows = rows;
            paint_state.initialized = true;
        }

        context
            .display_list
            .push(viewkit::draw_command::DrawCommand::PushClip { rect: content });
        Text::styled(paint_state.text.clone(), TextRole::Code)
            .color(context.theme.colors.text_primary)
            .paint(content, context);
        context
            .display_list
            .push(viewkit::draw_command::DrawCommand::PopClip);

        if output_changed {
            context.request_redraw_in_at(bounds, Instant::now());
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
