mod model;
mod session;
mod terminal_view;
#[cfg(target_os = "mochios")]
mod native_launch;
#[cfg(target_os = "mochios")]
mod stdio_access;
#[cfg(mochios_pty_launch_probe_310)]
#[path = "../../../services/posix/tests/support/pty_launch_terminal_310.rs"]
mod pty_launch_probe;

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
    #[cfg(mochios_pty_launch_probe_310)]
    pty_launch_probe::run();
    run::<TerminalApp>()
}
