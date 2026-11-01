mod input;
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
    type Body = ApplicationMenuBar<ui::TerminalUi>;
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
        let new = self.core.clone();
        let close = self.core.clone();
        let commands = self.core.clone();
        let settings = self.core.clone();
        let copy = self.core.clone();
        let paste = self.core.clone();
        let find = self.core.clone();
        ApplicationMenuBar::new(ui::TerminalUi {
            core: self.core.clone(),
        })
        .menu(
            ApplicationMenu::new("Terminal")
                .item(
                    ApplicationMenuItem::new("New Tab", move || {
                        let index = new.workspace.borrow().settings.borrow().default_profile;
                        new.queue(ui::Action::New(index));
                    })
                    .shortcut(MenuShortcut::command('t', "Ctrl+Shift+T").shift()),
                )
                .item(
                    ApplicationMenuItem::new("Close Pane", move || {
                        close.queue(ui::Action::ClosePane)
                    })
                    .shortcut(MenuShortcut::command('w', "Ctrl+Shift+W").shift()),
                )
                .separator()
                .item(
                    ApplicationMenuItem::new("Commands…", move || {
                        commands.queue(ui::Action::Commands)
                    })
                    .shortcut(MenuShortcut::command('p', "Ctrl+Shift+P").shift()),
                )
                .item(
                    ApplicationMenuItem::new("Settings…", move || {
                        settings.queue(ui::Action::Settings)
                    })
                    .shortcut(MenuShortcut::command(',', "Ctrl+,")),
                ),
        )
        .menu(
            ApplicationMenu::new("Edit")
                .item(
                    ApplicationMenuItem::new("Copy", move || copy.queue(ui::Action::Copy))
                        .shortcut(MenuShortcut::command('c', "Ctrl+Shift+C").shift()),
                )
                .item(
                    ApplicationMenuItem::new("Paste", move || paste.queue(ui::Action::Paste))
                        .shortcut(MenuShortcut::command('v', "Ctrl+Shift+V").shift()),
                )
                .item(
                    ApplicationMenuItem::new("Find…", move || find.queue(ui::Action::Find))
                        .shortcut(MenuShortcut::command('f', "Ctrl+Shift+F").shift()),
                ),
        )
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
