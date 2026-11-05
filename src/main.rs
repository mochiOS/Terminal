mod input;
#[cfg(target_os = "mochios")]
mod consent;
#[cfg(target_os = "linux")]
mod linux_launch;
mod model;
#[cfg(target_os = "mochios")]
mod native_launch;
mod pending_input;
#[cfg(mochios_pty_launch_probe_310)]
#[path = "../../../services/posix/tests/support/pty_launch_terminal_310.rs"]
mod pty_launch_probe;
mod session;
mod settings;
#[cfg(target_os = "mochios")]
mod stdio_access;
mod terminal_view;
mod transport;

mod ui;
mod workspace;
use appcore::prelude::*;
use std::rc::Rc;
struct TerminalApp {
    core: Rc<ui::Core>,
}
impl App for TerminalApp {
    type Body = ui::TerminalUi;
    fn new() -> Self {
        Self {
            core: ui::Core::new(),
        }
    }
    fn window(&self) -> WindowOptions {
        WindowOptions::new("Terminal")
            .size(1040.0, 700.0)
            .resizable(true)
    }
    fn body(&self, _: &ViewContext) -> Self::Body {
        ui::TerminalUi {
            core: self.core.clone(),
        }
    }
    #[cfg(target_os = "mochios")]
    fn protected_presentation_for_window(
        &self,
        window: WindowId,
    ) -> Option<viewkit::platform::ProtectedPresentation> {
        if window != WindowId::PRIMARY {
            return None;
        }
        self.core.consent.borrow().binding()
    }
    #[cfg(target_os = "mochios")]
    fn protected_presentation_failed(&mut self, _: WindowId) {
        self.core.consent.borrow_mut().withdraw();
    }
    fn close_requested(&mut self) -> bool {
        self.core.request_close()
    }
    fn close_requested_for_window(&mut self, _: WindowId) -> bool {
        self.core.request_close()
    }
}
fn main() -> Result<(), ViewKitError> {
    #[cfg(mochios_pty_launch_probe_310)]
    pty_launch_probe::run();
    appcore::run::<TerminalApp>()
}
