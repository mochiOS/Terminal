mod model;
mod session;
mod terminal_view;

use std::cell::RefCell;
use std::rc::Rc;

use session::TerminalSession;
use terminal_view::TerminalView;
use viewkit::prelude::*;

struct TerminalApp {
    session: Rc<RefCell<TerminalSession>>,
}

impl App for TerminalApp {
    type Body = TerminalView;

    fn new() -> Self {
        Self {
            session: Rc::new(RefCell::new(TerminalSession::start())),
        }
    }

    fn window(&self) -> WindowOptions {
        WindowOptions::new("Terminal")
            .compact()
            .resizable(true)
    }

    fn body(&self, _context: &ViewContext) -> Self::Body {
        TerminalView::new(Rc::clone(&self.session))
    }
}

fn main() -> Result<(), ViewKitError> {
    run::<TerminalApp>()
}
