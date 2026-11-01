//! Native terminal workspace chrome. PTYs, VT parsing and preferences live outside this module.
use crate::{
    settings::{self, Profile, Settings},
    workspace::Workspace,
};
use appcore::prelude::*;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    time::{Duration, Instant},
};
use viewkit::platform::PointerButton;
use viewkit::{
    event::{ContextMenuItem, ContextMenuRequest, EventContext, EventResult, ViewEvent},
    view::{Constraints, MeasureContext, PaintContext},
};
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Action {
    New(usize),
    Split(bool),
    ClosePane,
    CloseTab,
    CloseTabId(u64),
    Dropdown,
    NextTab(i8),
    MoveTab(i8),
    FocusNext,
    ZoomPane,
    Find,
    Search(bool),
    Copy,
    Paste,
    SelectAll,
    Clear,
    Zoom(i8),
    Commands,
    Settings,
    SaveSettings,
    Cancel,
    Confirm,
    EditProfile(usize),
    AddProfile,
    RemoveProfile,
    DefaultProfile,
    Rename,
    RenameApply,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    None,
    Commands,
    Settings,
    ClosePane,
    CloseTab,
    CloseWindow,
    Paste,
    Rename,
}
#[derive(Clone)]
struct Field {
    state: TextFieldInteractionState,
    view: Rc<TextField>,
}
impl Field {
    fn new(placeholder: &str) -> Self {
        let state = TextFieldInteractionState::new();
        Self {
            view: Rc::new(TextField::with_interaction(state.clone()).placeholder(placeholder)),
            state,
        }
    }
}
struct Choice {
    value: State<String>,
    options: Vec<String>,
    cached: RefCell<Option<(String, Rc<Picker>)>>,
}
impl Choice {
    fn new(value: &str, options: &[&str]) -> Rc<Self> {
        Rc::new(Self {
            value: State::new(value.into()),
            options: options.iter().map(|s| s.to_string()).collect(),
            cached: RefCell::new(None),
        })
    }
    fn picker(&self) -> Rc<Picker> {
        let value = self.value.get();
        if let Some((old, picker)) = self.cached.borrow().as_ref() {
            if *old == value {
                return picker.clone();
            }
        }
        let mut picker = Picker::new(&value);
        for option in &self.options {
            let state = self.value.clone();
            let option = option.clone();
            picker = picker.option(option.clone(), move || state.set(option.clone()));
        }
        let picker = Rc::new(picker);
        self.cached.replace(Some((value, picker.clone())));
        picker
    }
}
struct ChoiceView(Rc<Choice>);
impl View for ChoiceView {
    fn measure(&self, c: Constraints, m: &mut MeasureContext<'_>) -> Size {
        self.0.picker().measure(c, m)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        self.0.picker().paint(b, c)
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        self.0.picker().handle_event(b, e, c)
    }
}
struct Editor {
    draft: RefCell<Settings>,
    selected: Cell<usize>,
    name: Field,
    program: Field,
    args: Field,
    directory: Field,
    font: State<f32>,
    font_interaction: SliderInteractionState,
    history: Field,
    palette: Rc<Choice>,
    cursor: Rc<Choice>,
    copy: State<bool>,
    confirm: State<bool>,
    copy_interaction: SwitchInteractionState,
    confirm_interaction: SwitchInteractionState,
    scroll: ScrollState,
    profile_picker: RefCell<Option<(Vec<String>, usize, Rc<Picker>)>>,
}
impl Editor {
    fn new(settings: Settings) -> Self {
        let editor = Self {
            font: State::new(settings.font_size),
            history: Field::new("Lines"),
            palette: Choice::new(&settings.palette, &["System", "Light", "Dark"]),
            cursor: Choice::new(&settings.cursor, &["Bar", "Block", "Underline"]),
            copy: State::new(settings.copy_on_select),
            confirm: State::new(settings.confirm_close),
            draft: RefCell::new(settings),
            selected: Cell::new(0),
            name: Field::new("Profile name"),
            program: Field::new("Absolute executable path"),
            args: Field::new("Arguments (quotes supported)"),
            directory: Field::new("Inherit current directory"),
            font_interaction: SliderInteractionState::new(),
            copy_interaction: SwitchInteractionState::new(),
            confirm_interaction: SwitchInteractionState::new(),
            scroll: ScrollState::new(),
            profile_picker: RefCell::new(None),
        };
        editor
            .history
            .state
            .set_value(editor.draft.borrow().scrollback.to_string());
        editor.load_profile(0);
        editor
    }
    fn load_profile(&self, index: usize) {
        let draft = self.draft.borrow();
        let p = &draft.profiles[index];
        self.selected.set(index);
        self.name.state.set_value(&p.name);
        self.program.state.set_value(&p.program);
        self.args
            .state
            .set_value(settings::format_arguments(&p.arguments));
        self.directory.state.set_value(&p.directory);
    }
    fn flush(&self) -> Result<(), String> {
        let p = Profile {
            name: self.name.state.value(),
            program: self.program.state.value(),
            arguments: settings::arguments(&self.args.state.value())?,
            directory: self.directory.state.value(),
        };
        p.validate()?;
        self.draft.borrow_mut().profiles[self.selected.get()] = p;
        Ok(())
    }
    fn settings(&self) -> Result<Settings, String> {
        self.flush()?;
        let mut s = self.draft.borrow().clone();
        s.font_size = self.font.get();
        s.scrollback = self
            .history
            .state
            .value()
            .parse()
            .map_err(|_| "Enter a whole-number scrollback limit.")?;
        s.palette = self.palette.value.get();
        s.cursor = self.cursor.value.get();
        s.copy_on_select = self.copy.get();
        s.confirm_close = self.confirm.get();
        s.validate()?;
        Ok(s)
    }
}
pub(crate) struct Core {
    pub workspace: RefCell<Workspace>,
    pending: Rc<Cell<Option<Action>>>,
    buttons: RefCell<BTreeMap<(Action, u8), ButtonInteractionState>>,
    toolbar: ActionToolbarState<Action>,
    icons: RefCell<BTreeMap<Action, IconButtonInteractionState>>,
    open_menu: Cell<bool>,
    close_target: Cell<Option<u64>>,
    tabs: TabsInteractionState,
    tab_selected: State<usize>,
    tab_scroll: ScrollState,
    panel: Cell<Panel>,
    find_visible: Cell<bool>,
    find: Field,
    palette_query: Field,
    palette_scroll: ScrollState,
    palette_index: Cell<usize>,
    palette_last_query: RefCell<String>,
    rename: Field,
    editor: RefCell<Option<Editor>>,
    notice: RefCell<String>,
    pending_paste: RefCell<Option<(u64, String)>>,
    context: RefCell<Option<(u64, u64, u64, Vec<Action>)>>,
    drag: RefCell<Option<(Vec<bool>, bool, Rect)>>,
    last_activity: Cell<Instant>,
    pub exit_confirmed: Cell<bool>,
    focused: Cell<bool>,
    next_context: Cell<u64>,
}
impl Core {
    pub fn new() -> Rc<Self> {
        let (settings, warning) = Settings::load();
        Rc::new(Self {
            workspace: RefCell::new(Workspace::new(settings)),
            pending: Rc::new(Cell::new(None)),
            buttons: RefCell::new(BTreeMap::new()),
            toolbar: ActionToolbarState::new(),
            icons: RefCell::new(BTreeMap::new()),
            open_menu: Cell::new(false),
            close_target: Cell::new(None),
            tabs: TabsInteractionState::new(),
            tab_selected: State::new(1),
            tab_scroll: ScrollState::new(),
            panel: Cell::new(Panel::None),
            find_visible: Cell::new(false),
            find: Field::new("Find in scrollback"),
            palette_query: Field::new("Search commands or profiles"),
            palette_scroll: ScrollState::new(),
            palette_index: Cell::new(0),
            palette_last_query: RefCell::new(String::new()),
            rename: Field::new("Tab name (empty restores process title)"),
            editor: RefCell::new(None),
            notice: RefCell::new(warning.unwrap_or_default()),
            pending_paste: RefCell::new(None),
            context: RefCell::new(None),
            drag: RefCell::new(None),
            last_activity: Cell::new(Instant::now()),
            exit_confirmed: Cell::new(false),
            focused: Cell::new(true),
            next_context: Cell::new(0x5445_0000),
        })
    }
    fn interaction(&self, a: Action, place: u8) -> ButtonInteractionState {
        self.buttons
            .borrow_mut()
            .entry((a, place))
            .or_default()
            .clone()
    }
    fn button(&self, label: &str, a: Action) -> Button {
        let pending = self.pending.clone();
        Button::with_interaction_and_label(self.interaction(a.clone(), 0), label)
            .on_click(move || pending.set(Some(a.clone())))
    }
    fn icon(&self, symbol: SymbolName, label: &str, action: Action) -> IconButton {
        let state = self
            .icons
            .borrow_mut()
            .entry(action.clone())
            .or_default()
            .clone();
        let pending = self.pending.clone();
        IconButton::with_interaction(symbol, state)
            .accessibility_label(label)
            .on_click(move || pending.set(Some(action.clone())))
    }
    pub fn queue(&self, a: Action) {
        self.pending.set(Some(a));
    }
    pub fn request_close(&self) -> bool {
        if self.exit_confirmed.get()
            || !self.workspace.borrow().settings.borrow().confirm_close
            || !self
                .workspace
                .borrow()
                .all()
                .iter()
                .any(|p| p.session.borrow().running())
        {
            return true;
        }
        self.panel.set(Panel::CloseWindow);
        false
    }
    fn selected(&self) -> Option<Rc<crate::workspace::Pane>> {
        self.workspace.borrow().selected()
    }
    fn apply(&self) {
        let Some(action) = self.pending.take() else {
            return;
        };
        self.last_activity.set(Instant::now());
        let result = (|| -> Result<(), String> {
            match action {
                Action::New(index) => {
                    self.workspace.borrow_mut().new_tab(index)?;
                    self.panel.set(Panel::None);
                }
                Action::Split(horizontal) => {
                    self.workspace.borrow_mut().split(horizontal)?;
                    self.panel.set(Panel::None);
                }
                Action::Dropdown => {
                    self.open_menu.set(true);
                }
                Action::ClosePane | Action::CloseTab | Action::CloseTabId(_) => {
                    let target = match action {
                        Action::CloseTabId(id) => Some(id),
                        _ => self
                            .workspace
                            .borrow()
                            .tabs
                            .get(self.workspace.borrow().active)
                            .map(|t| t.id),
                    };
                    let confirm = self.workspace.borrow().settings.borrow().confirm_close;
                    let running = if action == Action::ClosePane {
                        self.selected()
                            .is_some_and(|p| p.session.borrow().running())
                    } else {
                        let w = self.workspace.borrow();
                        w.tabs
                            .iter()
                            .find(|t| Some(t.id) == target)
                            .and_then(|t| t.root.as_ref())
                            .is_some_and(|root| {
                                let mut panes = vec![];
                                root.leaves(&mut panes);
                                panes.iter().any(|p| p.session.borrow().running())
                            })
                    };
                    self.close_target.set(target);
                    if confirm && running {
                        self.panel.set(if action == Action::ClosePane {
                            Panel::ClosePane
                        } else {
                            Panel::CloseTab
                        });
                    } else {
                        if action == Action::ClosePane {
                            self.workspace.borrow_mut().close_pane();
                        } else {
                            if let Some(id) = target {
                                self.workspace.borrow_mut().close_tab_id(id);
                            }
                        }
                        self.panel.set(Panel::None);
                    }
                }
                Action::Confirm => {
                    match self.panel.get() {
                        Panel::Paste => {
                            if let Some((id, text)) = self.pending_paste.borrow_mut().take() {
                                let pane = self
                                    .workspace
                                    .borrow()
                                    .all()
                                    .into_iter()
                                    .find(|p| p.id == id)
                                    .ok_or("The target session was closed")?;
                                if !pane.session.borrow_mut().paste(&text) {
                                    return Err("Paste was not accepted by the session".into());
                                }
                            }
                        }
                        Panel::ClosePane => self.workspace.borrow_mut().close_pane(),
                        Panel::CloseTab => {
                            if let Some(id) = self.close_target.take() {
                                self.workspace.borrow_mut().close_tab_id(id);
                            }
                        }
                        Panel::CloseWindow => {
                            self.exit_confirmed.set(true);
                            request_quit();
                        }
                        _ => {}
                    }
                    self.panel.set(Panel::None);
                }
                Action::Cancel => {
                    self.close_target.set(None);
                    self.pending_paste.borrow_mut().take();
                    self.panel.set(Panel::None);
                    self.editor.borrow_mut().take();
                    self.palette_query.state.set_focused(false);
                }
                Action::NextTab(direction) => {
                    let mut w = self.workspace.borrow_mut();
                    if !w.tabs.is_empty() {
                        w.active = (w.active as isize + direction as isize)
                            .rem_euclid(w.tabs.len() as isize)
                            as usize;
                        w.zoomed = false;
                    }
                }
                Action::MoveTab(direction) => {
                    let mut w = self.workspace.borrow_mut();
                    let other = w.active as isize + direction as isize;
                    if other >= 0 && (other as usize) < w.tabs.len() {
                        let active = w.active;
                        w.tabs.swap(active, other as usize);
                        w.active = other as usize;
                    }
                }
                Action::FocusNext => self.workspace.borrow_mut().focus_next(),
                Action::ZoomPane => {
                    let mut w = self.workspace.borrow_mut();
                    w.zoomed = !w.zoomed;
                }
                Action::Find => {
                    self.find_visible.set(!self.find_visible.get());
                    self.find.state.set_focused(self.find_visible.get());
                    self.panel.set(Panel::None);
                }
                Action::Search(previous) => {
                    if let Some(pane) = self.selected() {
                        let found = pane
                            .session
                            .borrow_mut()
                            .buffer
                            .find(&self.find.state.value(), previous);
                        pane.view.highlight.set(found);
                        *self.notice.borrow_mut() = if found.is_some() {
                            "Match found in scrollback".into()
                        } else {
                            "No matching text".into()
                        };
                    }
                }
                Action::Copy => {
                    if let Some(p) = self.selected() {
                        p.view.copy();
                    }
                    self.panel.set(Panel::None);
                }
                Action::Paste => {
                    let text = viewkit::platform::clipboard::text()
                        .ok_or("No text is available in the session clipboard.")?;
                    self.prepare_paste(text)?;
                }
                Action::SelectAll => {
                    if let Some(p) = self.selected() {
                        p.view.select_all();
                    }
                    self.panel.set(Panel::None);
                }
                Action::Clear => {
                    if let Some(p) = self.selected() {
                        p.view.clear_selection();
                        p.session.borrow_mut().buffer.clear();
                    }
                    self.panel.set(Panel::None);
                }
                Action::Zoom(change) => {
                    let settings = self.workspace.borrow().settings.clone();
                    let mut s = settings.borrow_mut();
                    s.font_size = if change == 0 {
                        13.0
                    } else {
                        (s.font_size + change as f32).clamp(10.0, 32.0)
                    };
                }
                Action::Commands => {
                    self.panel.set(Panel::Commands);
                    self.palette_query.state.set_value("");
                    self.palette_query.state.set_focused(true);
                    self.palette_scroll.reset();
                    self.palette_index.set(0);
                }
                Action::Settings => {
                    self.notice.borrow_mut().clear();
                    let s = self.workspace.borrow().settings.borrow().clone();
                    self.editor.replace(Some(Editor::new(s)));
                    self.panel.set(Panel::Settings);
                }
                Action::SaveSettings => {
                    let settings = self
                        .editor
                        .borrow()
                        .as_ref()
                        .ok_or("Settings are not open")?
                        .settings()?;
                    settings.save_to(&Settings::path())?;
                    *self.workspace.borrow().settings.borrow_mut() = settings;
                    self.editor.borrow_mut().take();
                    self.panel.set(Panel::None);
                    *self.notice.borrow_mut() =
                        "Settings saved. Profiles and scrollback limits apply to new sessions."
                            .into();
                }
                Action::EditProfile(index) => {
                    let e = self.editor.borrow();
                    let e = e.as_ref().ok_or("Settings are not open")?;
                    e.flush()?;
                    if index < e.draft.borrow().profiles.len() {
                        e.load_profile(index);
                    }
                }
                Action::AddProfile => {
                    let e = self.editor.borrow();
                    let e = e.as_ref().ok_or("Settings are not open")?;
                    e.flush()?;
                    let mut draft = e.draft.borrow_mut();
                    if draft.profiles.len() >= 12 {
                        return Err("A maximum of 12 profiles is supported.".into());
                    }
                    let index = draft.profiles.len();
                    let mut p = Profile::default();
                    p.name = format!("Shell {}", index + 1);
                    draft.profiles.push(p);
                    drop(draft);
                    e.load_profile(index);
                }
                Action::RemoveProfile => {
                    let e = self.editor.borrow();
                    let e = e.as_ref().ok_or("Settings are not open")?;
                    let mut draft = e.draft.borrow_mut();
                    if draft.profiles.len() <= 1 {
                        return Err("Keep at least one profile.".into());
                    }
                    let index = e.selected.get();
                    draft.profiles.remove(index);
                    draft.default_profile = if draft.default_profile == index {
                        0
                    } else if draft.default_profile > index {
                        draft.default_profile - 1
                    } else {
                        draft.default_profile
                    };
                    drop(draft);
                    e.load_profile(0);
                }
                Action::DefaultProfile => {
                    let e = self.editor.borrow();
                    let e = e.as_ref().ok_or("Settings are not open")?;
                    e.draft.borrow_mut().default_profile = e.selected.get();
                }
                Action::Rename => {
                    if let Some(tab) = self
                        .workspace
                        .borrow()
                        .tabs
                        .get(self.workspace.borrow().active)
                    {
                        self.rename
                            .state
                            .set_value(tab.name.clone().unwrap_or_default());
                        self.rename.state.set_focused(true);
                        self.panel.set(Panel::Rename);
                    }
                }
                Action::RenameApply => {
                    let name = self.rename.state.value();
                    if name.len() > 80 || name.chars().any(char::is_control) {
                        return Err(
                            "Use a tab name of up to 80 bytes without control characters.".into(),
                        );
                    }
                    let mut w = self.workspace.borrow_mut();
                    let index = w.active;
                    if let Some(tab) = w.tabs.get_mut(index) {
                        tab.name = if name.trim().is_empty() {
                            None
                        } else {
                            Some(name)
                        };
                    }
                    self.panel.set(Panel::None);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            *self.notice.borrow_mut() = error;
        }
        let w = self.workspace.borrow();
        self.tab_selected
            .set(w.tabs.get(w.active).map(|t| t.id as usize).unwrap_or(0));
    }
    fn prepare_paste(&self, text: String) -> Result<(), String> {
        self.panel.set(Panel::None);
        if let Some(p) = self.selected() {
            if text.len() > 60_000 {
                return Err(
                    "Paste is limited to 60,000 bytes. Use a file for larger input.".into(),
                );
            }
            if text.contains(['\n', '\r']) {
                self.pending_paste.replace(Some((p.id, text)));
                self.panel.set(Panel::Paste);
            } else if !p.session.borrow_mut().paste(&text) {
                return Err("Paste was not accepted by the session.".into());
            }
        }
        Ok(())
    }
    fn filtered_commands(&self) -> Vec<(Action, String, String)> {
        let query = self.palette_query.state.value().to_lowercase();
        if *self.palette_last_query.borrow() != query {
            self.palette_last_query.replace(query.clone());
            self.palette_index.set(0);
            self.palette_scroll.reset();
        }
        self.commands()
            .into_iter()
            .filter(|(_, label, _)| label.to_lowercase().contains(&query))
            .collect()
    }
    fn commands(&self) -> Vec<(Action, String, String)> {
        let mut commands = vec![
            (
                Action::New(self.workspace.borrow().settings.borrow().default_profile),
                "New tab".into(),
                "Ctrl+Shift+T".into(),
            ),
            (
                Action::Split(true),
                "Split right".into(),
                "Ctrl+Shift+D".into(),
            ),
            (
                Action::Split(false),
                "Split down".into(),
                "Ctrl+Alt+D".into(),
            ),
            (
                Action::ClosePane,
                "Close active pane".into(),
                "Ctrl+Shift+W".into(),
            ),
            (Action::CloseTab, "Close tab".into(), "Ctrl+Alt+W".into()),
            (Action::NextTab(1), "Next tab".into(), "Ctrl+Tab".into()),
            (
                Action::NextTab(-1),
                "Previous tab".into(),
                "Ctrl+Shift+Tab".into(),
            ),
            (
                Action::FocusNext,
                "Focus next pane".into(),
                "Alt+Arrow".into(),
            ),
            (
                Action::ZoomPane,
                "Zoom active pane".into(),
                "Ctrl+Shift+Z".into(),
            ),
            (
                Action::Find,
                "Find in scrollback".into(),
                "Ctrl+Shift+F".into(),
            ),
            (Action::Copy, "Copy selection".into(), "Ctrl+Shift+C".into()),
            (Action::Paste, "Paste".into(), "Ctrl+Shift+V".into()),
            (
                Action::SelectAll,
                "Select visible output".into(),
                "Ctrl+Shift+A".into(),
            ),
            (
                Action::Clear,
                "Clear screen and scrollback".into(),
                "Ctrl+Shift+K".into(),
            ),
            (
                Action::Zoom(1),
                "Increase text size".into(),
                "Ctrl++".into(),
            ),
            (
                Action::Zoom(-1),
                "Decrease text size".into(),
                "Ctrl+-".into(),
            ),
            (Action::Zoom(0), "Reset text size".into(), "Ctrl+0".into()),
            (Action::Rename, "Rename tab".into(), String::new()),
            (
                Action::Settings,
                "Settings and profiles".into(),
                "Ctrl+,".into(),
            ),
        ];
        for (index, profile) in self
            .workspace
            .borrow()
            .settings
            .borrow()
            .profiles
            .iter()
            .enumerate()
        {
            commands.push((
                Action::New(index),
                format!("New tab: {}", profile.name),
                String::new(),
            ));
        }
        commands
    }
    fn tabstrip(&self) -> HStack {
        let w = self.workspace.borrow();
        let width = Theme::current().layout.navigation_sidebar_width;
        let mut tabs = Tabs::with_interaction(self.tab_selected.binding(), self.tabs.clone())
            .item_width(width)
            .accessibility_label("Terminal tabs");
        for tab in &w.tabs {
            let title = tab.name.clone().unwrap_or_else(|| {
                let mut leaves = vec![];
                if let Some(root) = &tab.root {
                    root.leaves(&mut leaves);
                }
                leaves
                    .iter()
                    .find(|p| p.id == tab.active)
                    .map(|p| p.session.borrow().title())
                    .unwrap_or_else(|| "Terminal".into())
            });
            let pending = self.pending.clone();
            let id = tab.id;
            tabs = tabs.closable_item(
                tab.id as usize,
                title.chars().take(30).collect::<String>(),
                move || pending.set(Some(Action::CloseTabId(id))),
            );
        }
        let default = w.settings.borrow().default_profile;
        HStack::new()
            .alignment(StackAlignment::Center)
            .gap(StackGap::ExtraSmall)
            .child(
                Scroll::new(self.tab_scroll.clone())
                    .axis(ScrollAxis::Horizontal)
                    .content(tabs)
                    .layout()
                    .width(width * w.tabs.len() as f32)
                    .flex_shrink(1.0),
            )
            .child(
                self.icon(
                    SymbolName::Plus,
                    "New tab (Ctrl+Shift+T)",
                    Action::New(default),
                )
                .layout()
                .flex_shrink(0.0),
            )
            .child(
                self.icon(
                    SymbolName::ChevronDown,
                    "Profiles and terminal actions",
                    Action::Dropdown,
                )
                .layout()
                .flex_shrink(0.0),
            )
            .child(Spacer::new())
    }
    fn show_dropdown(&self, position: Point, c: &mut EventContext<'_>) {
        let w = self.workspace.borrow();
        let mut actions = vec![];
        let mut items = vec![];
        for (index, profile) in w.settings.borrow().profiles.iter().enumerate() {
            actions.push(Action::New(index));
            items.push(ContextMenuItem {
                command_id: actions.len() as u32,
                label: format!("New {} tab", profile.name),
                enabled: true,
                checked: false,
                destructive: false,
                separator: false,
            });
        }
        items.push(ContextMenuItem {
            command_id: 0,
            label: String::new(),
            enabled: false,
            checked: false,
            destructive: false,
            separator: true,
        });
        for (action, label) in [
            (Action::Split(true), "Split right"),
            (Action::Split(false), "Split down"),
            (Action::Find, "Find…"),
            (Action::Commands, "Command palette…"),
            (Action::Settings, "Settings…"),
            (Action::ClosePane, "Close pane"),
            (Action::CloseTab, "Close tab"),
        ] {
            let enabled =
                !w.tabs.is_empty() || matches!(action, Action::Commands | Action::Settings);
            let destructive = matches!(action, Action::ClosePane | Action::CloseTab);
            actions.push(action);
            items.push(ContextMenuItem {
                command_id: actions.len() as u32,
                label: label.into(),
                enabled,
                checked: false,
                destructive,
                separator: false,
            });
        }
        let id = self.next_context.get() + 1;
        self.next_context.set(id);
        let (tab, pane) = w
            .tabs
            .get(w.active)
            .map(|t| (t.id, t.active))
            .unwrap_or((0, 0));
        self.context.replace(Some((id, tab, pane, actions)));
        c.show_context_menu(ContextMenuRequest {
            request_id: id,
            position,
            items,
        });
    }
    fn finder(&self) -> VStack {
        VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::Small)
            .child(self.find.view.clone())
            .child(
                HStack::new()
                    .alignment(StackAlignment::Center)
                    .gap(StackGap::Small)
                    .child(self.button("Previous", Action::Search(true)))
                    .child(self.button("Next", Action::Search(false)))
                    .child(Spacer::new())
                    .child(self.button("Close Find", Action::Find)),
            )
    }
    fn settings_page(&self) -> Box<dyn View> {
        let slot = self.editor.borrow();
        let Some(e) = slot.as_ref() else {
            return Box::new(Text::body("Settings are not available."));
        };
        let names: Vec<_> = e
            .draft
            .borrow()
            .profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let index = e.selected.get();
        let picker = if let Some((old, old_index, picker)) = e.profile_picker.borrow().as_ref() {
            if *old == names && *old_index == index {
                Some(picker.clone())
            } else {
                None
            }
        } else {
            None
        };
        let picker = picker.unwrap_or_else(|| {
            let mut picker = Picker::new(&names[index]);
            for (i, name) in names.iter().enumerate() {
                let pending = self.pending.clone();
                picker = picker.option(name, move || pending.set(Some(Action::EditProfile(i))));
            }
            let picker = Rc::new(picker);
            e.profile_picker
                .replace(Some((names, index, picker.clone())));
            picker
        });
        let mut rows=VStack::new().alignment(StackAlignment::Stretch).gap(StackGap::Large)
            .child(Text::styled("Settings",TextRole::TitleSmall))
            .child(Text::caption("Save to apply appearance changes. Shell profiles and scrollback apply to new sessions."))
            .child(SettingsSection::new("Appearance").grouped()
                .row(SettingsRow::new("Text size",Slider::with_interaction(e.font.binding(),e.font_interaction.clone()).range(10.0..=32.0).step(1.0)).compact())
                .row(SettingsRow::new("Color scheme",ChoiceView(e.palette.clone())).compact())
                .row(SettingsRow::new("Cursor",ChoiceView(e.cursor.clone())).compact())
                .row(SettingsRow::new("Scrollback lines",e.history.view.clone()).compact())
                .row(SettingsRow::new("Copy on selection",Switch::with_interaction(e.copy.binding(),e.copy_interaction.clone())).compact())
                .row(SettingsRow::new("Confirm closing running sessions",Switch::with_interaction(e.confirm.binding(),e.confirm_interaction.clone())).compact()))
            .child(SettingsSection::new("Profiles").grouped()
                .row(SettingsRow::new("Edit profile",picker).compact())
                .row(SettingsRow::new("Name",e.name.view.clone().layout().width(Theme::current().layout.form_width * 0.6)).compact())
                .row(SettingsRow::new("Executable",e.program.view.clone().layout().width(Theme::current().layout.form_width * 0.6)).compact())
                .row(SettingsRow::new("Arguments",e.args.view.clone().layout().width(Theme::current().layout.form_width * 0.6)).description("Quotes group arguments. No shell expansion is performed.").compact()));
        #[cfg(target_os = "linux")]
        {
            rows = rows.child(
                SettingsRow::new(
                    "Starting directory",
                    e.directory
                        .view
                        .clone()
                        .layout()
                        .width(Theme::current().layout.form_width * 0.6),
                )
                .description("Leave empty to inherit.")
                .compact(),
            );
        }
        if !self.notice.borrow().is_empty() {
            rows = rows.child(Text::body(self.notice.borrow().clone()));
        }
        rows = rows
            .child(
                HStack::new()
                    .gap(StackGap::Small)
                    .child(self.button("Add Profile", Action::AddProfile))
                    .child(self.button("Set as Default", Action::DefaultProfile))
                    .child(
                        self.button("Remove Profile", Action::RemoveProfile)
                            .enabled(e.draft.borrow().profiles.len() > 1),
                    ),
            )
            .child(Text::caption(format!(
                "Default profile: {}",
                e.draft.borrow().profiles[e.draft.borrow().default_profile].name
            )));
        Box::new(
            VStack::new()
                .alignment(StackAlignment::Stretch)
                .gap(StackGap::None)
                .child(Toolbar::new(
                    ActionToolbar::new(self.toolbar.clone())
                        .title("Settings")
                        .leading(Action::Cancel, self.button("Cancel", Action::Cancel))
                        .trailing(
                            Action::SaveSettings,
                            self.button("Save", Action::SaveSettings),
                        ),
                ))
                .child(
                    Surface::pane()
                        .content(
                            Scroll::new(e.scroll.clone())
                                .content(ContentArea::pane(Form::new(rows))),
                        )
                        .layout()
                        .flex_grow(1.0)
                        .flex_shrink(1.0),
                )
                .child(
                    Padding::all(Theme::current().spacing.small)
                        .content(Text::caption(self.notice.borrow().clone()).single_line(true)),
                ),
        )
    }
    fn dialog(&self) -> Dialog<VStack> {
        let panel = self.panel.get();
        let mut content = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::Medium);
        if panel == Panel::Commands {
            let mut list = List::new();
            let matches = self.filtered_commands();
            let empty = matches.is_empty();
            for (index, (action, label, shortcut)) in
                self.filtered_commands().into_iter().enumerate()
            {
                let pending = self.pending.clone();
                list = list.row(
                    ListRow::with_interaction(self.interaction(action.clone(), 1), label)
                        .selected(index == self.palette_index.get())
                        .trailing(shortcut)
                        .on_select(move || pending.set(Some(action.clone())))
                        .layout()
                        .height(Theme::current().layout.settings_row_height),
                );
            }
            content = content
                .child(Text::styled("Commands", TextRole::TitleSmall))
                .child(Text::caption(if empty {
                    "No commands match your search."
                } else {
                    "↑↓ choose · Enter run · Esc close"
                }))
                .child(self.palette_query.view.clone())
                .child(
                    Scroll::new(self.palette_scroll.clone())
                        .content(
                            Padding::only(0.0, Theme::current().spacing.medium, 0.0, 0.0)
                                .content(list),
                        )
                        .layout()
                        .flex_grow(1.0)
                        .flex_shrink(1.0),
                )
                .child(self.button("Close", Action::Cancel));
        } else if panel == Panel::Paste {
            content = content.child(Text::styled("Paste multiple lines?",TextRole::TitleSmall))
                .child(Text::body("This text contains line breaks and may run commands immediately in your shell. Paste only text you trust."))
                .child(HStack::new().gap(StackGap::Small).child(self.button("Cancel",Action::Cancel)).child(self.button("Paste",Action::Confirm)));
        } else if panel == Panel::Rename {
            content = content
                .child(Text::styled("Rename Tab", TextRole::TitleSmall))
                .child(self.rename.view.clone())
                .child(
                    HStack::new()
                        .gap(StackGap::Small)
                        .child(self.button("Cancel", Action::Cancel))
                        .child(self.button("Rename", Action::RenameApply)),
                );
        } else {
            content=content.child(Text::styled("Close running sessions?",TextRole::TitleSmall)).child(Text::body("Processes in the selected pane or tab will be terminated. Terminal output is not restored after closing."))
            .child(HStack::new().gap(StackGap::Small).child(self.button("Cancel",Action::Cancel)).child(self.button("Close",Action::Confirm).style(ButtonStyle::Danger)));
        }
        Dialog::new()
            .accessibility_label(if panel == Panel::Commands {
                "Terminal commands"
            } else {
                "Terminal confirmation"
            })
            .content(content)
    }
}
#[derive(Clone)]
pub(crate) struct TerminalUi {
    pub core: Rc<Core>,
}
#[derive(Clone, Copy)]
struct Frames {
    tabs: Rect,
    find: Option<Rect>,
    body: Rect,
    status: Rect,
}
impl TerminalUi {
    fn frames(&self, b: Rect, c: &mut MeasureContext<'_>) -> Frames {
        let top = 0.0;
        let tab = c.theme.tabs.height.min((b.size.height - top).max(0.0));
        let status = if !self.core.notice.borrow().is_empty() || self.core.find_visible.get() {
            c.theme
                .layout
                .status_bar_height
                .min((b.size.height - top - tab).max(0.0))
        } else {
            0.0
        };
        let find_height = if self.core.find_visible.get() {
            Toolbar::new(self.core.finder())
                .measure(
                    Constraints::loose(Size::new(b.size.width, b.size.height)),
                    c,
                )
                .height
                .min((b.size.height - top - tab - status).max(0.0))
        } else {
            0.0
        };
        Frames {
            tabs: Rect::new(b.origin.x, b.origin.y + top, b.size.width, tab),
            find: self.core.find_visible.get().then(|| {
                Rect::new(
                    b.origin.x,
                    b.origin.y + top + tab,
                    b.size.width,
                    find_height,
                )
            }),
            body: Rect::new(
                b.origin.x,
                b.origin.y + top + tab + find_height,
                b.size.width,
                (b.size.height - top - tab - find_height - status).max(0.0),
            ),
            status: Rect::new(
                b.origin.x,
                b.origin.y + b.size.height - status,
                b.size.width,
                status,
            ),
        }
    }
    fn modal_bounds(&self, b: Rect) -> Rect {
        let height = if self.core.panel.get() == Panel::Commands {
            (b.size.height - 48.0).clamp(0.0, 520.0)
        } else {
            240.0_f32.min((b.size.height - 32.0).max(0.0))
        };
        let width = (b.size.width - 32.0).clamp(0.0, 560.0);
        Rect::new(
            b.origin.x + (b.size.width - width) * 0.5,
            b.origin.y + (b.size.height - height) * 0.5,
            width,
            height,
        )
    }
    fn terminal_bounds(b: Rect, split: bool, theme: &Theme) -> Rect {
        let header = if split {
            theme.controls.small_height.min(b.size.height)
        } else {
            0.0
        };
        Rect::new(
            b.origin.x,
            b.origin.y + header,
            b.size.width,
            (b.size.height - header).max(0.0),
        )
    }
    fn key(&self, key: Key, m: KeyModifiers) -> Option<Action> {
        if m.shortcut() {
            match key {
                Key::Tab => return Some(Action::NextTab(if m.shift() { -1 } else { 1 })),
                Key::PageUp if m.shift() => return Some(Action::MoveTab(-1)),
                Key::PageDown if m.shift() => return Some(Action::MoveTab(1)),
                Key::Character(c) => {
                    let c = c.to_ascii_lowercase();
                    return match (c, m.shift(), m.alt()) {
                        ('t', true, _) => Some(Action::New(
                            self.core
                                .workspace
                                .borrow()
                                .settings
                                .borrow()
                                .default_profile,
                        )),
                        ('d', true, _) => Some(Action::Split(true)),
                        ('d', _, true) => Some(Action::Split(false)),
                        ('w', true, _) => Some(Action::ClosePane),
                        ('w', _, true) => Some(Action::CloseTab),
                        ('p', true, _) => Some(Action::Commands),
                        ('f', true, _) => Some(Action::Find),
                        ('c', true, _) => Some(Action::Copy),
                        ('v', true, _) => Some(Action::Paste),
                        ('a', true, _) => Some(Action::SelectAll),
                        ('k', true, _) => Some(Action::Clear),
                        ('z', true, _) => Some(Action::ZoomPane),
                        (',', _, _) => Some(Action::Settings),
                        ('+' | '=', _, _) => Some(Action::Zoom(1)),
                        ('-', _, _) => Some(Action::Zoom(-1)),
                        ('0', _, _) => Some(Action::Zoom(0)),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
        if m.alt()
            && !m.control()
            && matches!(
                key,
                Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown
            )
            && self.core.workspace.borrow().all().len() > 1
        {
            Some(Action::FocusNext)
        } else {
            None
        }
    }
}
impl View for TerminalUi {
    fn measure(&self, c: Constraints, _: &mut MeasureContext<'_>) -> Size {
        c.constrain(c.maximum)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        self.core.apply();
        let panes = self.core.workspace.borrow().all();
        let mut running = false;
        for pane in &panes {
            let mut session = pane.session.borrow_mut();
            if session.poll() {
                self.core.last_activity.set(Instant::now());
            }
            running |= session.needs_poll();
            if session.buffer.take_bell() {
                *self.core.notice.borrow_mut() = "Bell in a terminal session".into();
            }
        }
        if running {
            c.request_redraw_in_at(
                b,
                Instant::now()
                    + Duration::from_millis(
                        if self.core.last_activity.get().elapsed() < Duration::from_secs(1) {
                            16
                        } else {
                            80
                        },
                    ),
            );
        }
        if self.core.panel.get() == Panel::Settings {
            Surface::app()
                .content(self.core.settings_page())
                .paint(b, c);
            return;
        }
        let frames = self.frames(
            b,
            &mut MeasureContext {
                theme: c.theme,
                typography: c.typography,
                text_measurer: c.text_measurer,
            },
        );
        Surface::app().paint(b, c);
        Surface::sidebar()
            .content(self.core.tabstrip())
            .paint(frames.tabs, c);
        if let Some(find) = frames.find {
            Toolbar::new(self.core.finder()).paint(find, c);
        }
        let w = self.core.workspace.borrow();
        let active = w.tabs.get(w.active).map(|tab| tab.active);
        let pane_frames = w.frames(frames.body, c.theme.spacing.extra_small);
        let split = pane_frames.len() > 1;
        if pane_frames.is_empty() {
            ContentArea::pane(
                VStack::new()
                    .gap(StackGap::Medium)
                    .child(Text::styled("No open sessions", TextRole::TitleSmall))
                    .child(Text::body("Open a new tab to start a shell."))
                    .child(
                        self.core
                            .button("New Tab", Action::New(w.settings.borrow().default_profile)),
                    ),
            )
            .paint(frames.body, c);
        }
        for (pane, frame) in pane_frames {
            pane.view.focused.set(
                active == Some(pane.id)
                    && self.core.panel.get() == Panel::None
                    && !self.core.find.state.is_focused(),
            );
            if split {
                Surface::sidebar().paint(
                    Rect::new(
                        frame.origin.x,
                        frame.origin.y,
                        frame.size.width,
                        c.theme.controls.small_height,
                    ),
                    c,
                );
                Text::caption(format!(
                    "{}{}",
                    if active == Some(pane.id) { "● " } else { "" },
                    pane.session.borrow().title()
                ))
                .single_line(true)
                .paint(
                    Rect::new(
                        frame.origin.x + c.theme.spacing.small,
                        frame.origin.y,
                        (frame.size.width - c.theme.spacing.small * 2.0).max(0.0),
                        c.theme.controls.small_height,
                    ),
                    c,
                );
            }
            let terminal = Self::terminal_bounds(frame, split, c.theme);
            Surface::pane()
                .in_container(b)
                .content(pane.view.clone())
                .paint(terminal, c);
        }
        drop(w);
        let status = if !self.core.notice.borrow().is_empty() {
            self.core.notice.borrow().clone()
        } else if self.core.find_visible.get() {
            "Enter: search · Shift+Enter: previous · Esc: close find".into()
        } else if let Some(p) = self.core.selected() {
            let session = p.session.borrow();
            let (rows, cols) = session.buffer.screen().size();
            format!(
                "{} · {} × {} · {} · Ctrl+Shift+P: commands",
                session.profile.name,
                cols,
                rows,
                if session.running() {
                    "Running"
                } else {
                    "Exited"
                }
            )
        } else {
            "Ctrl+Shift+T: new tab · Ctrl+Shift+P: commands".into()
        };
        Surface::sidebar()
            .content(
                Padding::symmetric(c.theme.spacing.large, 0.0)
                    .content(Text::caption(status).single_line(true)),
            )
            .paint(frames.status, c);
        if self.core.panel.get() != Panel::None {
            Rectangle::new()
                .color(RectangleColor::Custom(c.theme.shell.scrim))
                .paint(b, c);
            self.core.dialog().paint(self.modal_bounds(b), c);
        }
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        if let ViewEvent::FocusChanged { focused } = e {
            self.core.focused.set(*focused);
            if !focused {
                self.core.drag.borrow_mut().take();
            }
        }
        if self.core.panel.get() != Panel::None {
            if matches!(
                e,
                ViewEvent::KeyPressed {
                    key: Key::Escape,
                    ..
                }
            ) {
                self.core.queue(Action::Cancel);
            } else if self.core.panel.get() == Panel::Commands
                && matches!(
                    e,
                    ViewEvent::KeyPressed {
                        key: Key::Enter,
                        ..
                    }
                )
            {
                if let Some((action, _, _)) = self
                    .core
                    .filtered_commands()
                    .into_iter()
                    .nth(self.core.palette_index.get())
                {
                    self.core.panel.set(Panel::None);
                    self.core.queue(action);
                }
            } else if self.core.panel.get() == Panel::Commands
                && matches!(
                    e,
                    ViewEvent::KeyPressed {
                        key: Key::ArrowUp | Key::ArrowDown,
                        ..
                    }
                )
            {
                let count = self.core.filtered_commands().len();
                if count > 0 {
                    let previous = matches!(
                        e,
                        ViewEvent::KeyPressed {
                            key: Key::ArrowUp,
                            ..
                        }
                    );
                    let index = (self.core.palette_index.get() as isize
                        + if previous { -1 } else { 1 })
                    .rem_euclid(count as isize) as usize;
                    self.core.palette_index.set(index);
                    let row = c.theme().layout.settings_row_height;
                    let viewport = (self.modal_bounds(b).size.height - 180.0).max(row);
                    let offset = self.core.palette_scroll.offset_y();
                    if index as f32 * row < offset {
                        self.core.palette_scroll.set_offset(0.0, index as f32 * row);
                    } else if (index + 1) as f32 * row > offset + viewport {
                        self.core
                            .palette_scroll
                            .set_offset(0.0, ((index + 1) as f32 * row - viewport).max(0.0));
                    }
                }
            } else if self.core.panel.get() == Panel::Settings {
                self.core.settings_page().handle_event(b, e, c);
            } else {
                self.core.dialog().handle_event(self.modal_bounds(b), e, c);
            }
            self.core.apply();
            c.request_redraw_in(b);
            return EventResult::Consumed;
        }
        if let ViewEvent::KeyPressed { key, modifiers } = e {
            if *key == Key::Escape && self.core.find_visible.get() {
                self.core.find_visible.set(false);
                self.core.find.state.set_focused(false);
                if let Some(p) = self.core.selected() {
                    p.view.focused.set(true);
                }
                c.request_redraw_in(b);
                return EventResult::Consumed;
            }
            if self.core.find_visible.get()
                && self.core.find.state.is_focused()
                && *key == Key::Enter
            {
                self.core.queue(Action::Search(modifiers.shift()));
                self.core.apply();
                c.request_redraw_in(b);
                return EventResult::Consumed;
            }
            if let Some(action) = self.key(*key, *modifiers) {
                self.core.queue(action);
                self.core.apply();
                c.request_redraw_in(b);
                return EventResult::Consumed;
            }
        }
        let frames = self.frames(b, &mut c.measure_context());
        if let ViewEvent::ContextMenuResult {
            request_id,
            command_id,
        } = e
        {
            if let Some((id, tab, pane, actions)) = self.core.context.borrow_mut().take() {
                let w = self.core.workspace.borrow();
                if id == *request_id
                    && ((tab == 0 && w.tabs.is_empty())
                        || w.tabs
                            .get(w.active)
                            .is_some_and(|t| t.id == tab && t.active == pane))
                {
                    if let Some(action) =
                        command_id.and_then(|id| actions.get(id.saturating_sub(1) as usize))
                    {
                        self.core.queue(action.clone());
                    }
                }
            }
        }
        let mut dividers = vec![];
        {
            let w = self.core.workspace.borrow();
            if !w.zoomed {
                if let Some(root) = w.tabs.get(w.active).and_then(|t| t.root.as_ref()) {
                    root.dividers(
                        frames.body,
                        c.theme().spacing.extra_small,
                        vec![],
                        &mut dividers,
                    );
                }
            }
        }
        if let ViewEvent::PointerPressed {
            position,
            button: PointerButton::Primary,
        } = e
        {
            if let Some((path, _, horizontal, area)) = dividers
                .iter()
                .find(|(_, divider, _, _)| divider.contains(*position))
            {
                self.core
                    .drag
                    .replace(Some((path.clone(), *horizontal, *area)));
                return EventResult::Consumed;
            }
        }
        if let ViewEvent::PointerMoved { position } = e {
            if let Some((path, horizontal, area)) = self.core.drag.borrow().as_ref() {
                let ratio = if *horizontal {
                    (position.x - area.origin.x) / area.size.width.max(1.0)
                } else {
                    (position.y - area.origin.y) / area.size.height.max(1.0)
                };
                let mut w = self.core.workspace.borrow_mut();
                let active = w.active;
                if let Some(root) = w.tabs.get_mut(active).and_then(|t| t.root.as_mut()) {
                    root.resize_split(path, ratio);
                }
                c.request_redraw_in(frames.body);
                return EventResult::Consumed;
            }
        }
        if matches!(e, ViewEvent::PointerReleased { .. }) {
            self.core.drag.borrow_mut().take();
        }
        let pane_frames = self
            .core
            .workspace
            .borrow()
            .frames(frames.body, c.theme().spacing.extra_small);
        let split = pane_frames.len() > 1;
        if let ViewEvent::PointerPressed { position, button } = e {
            if let Some((pane, _)) = pane_frames
                .iter()
                .find(|(_, frame)| frame.contains(*position))
            {
                {
                    let mut w = self.core.workspace.borrow_mut();
                    let active = w.active;
                    w.tabs[active].active = pane.id;
                }
                self.core.find.state.set_focused(false);
                self.core.notice.borrow_mut().clear();
                if *button == PointerButton::Secondary && !pane.view.reports_mouse(c.modifiers()) {
                    let w = self.core.workspace.borrow();
                    let actions = vec![
                        Action::Copy,
                        Action::Paste,
                        Action::Find,
                        Action::Split(true),
                        Action::Split(false),
                        Action::ZoomPane,
                        Action::Rename,
                        Action::ClosePane,
                        Action::CloseTab,
                    ];
                    let labels = [
                        "Copy",
                        "Paste",
                        "Find…",
                        "Split Right",
                        "Split Down",
                        "Zoom Pane",
                        "Rename Tab…",
                        "Close Pane",
                        "Close Tab",
                    ];
                    let id = self.core.next_context.get() + 1;
                    self.core.next_context.set(id);
                    self.core
                        .context
                        .replace(Some((id, w.tabs[w.active].id, pane.id, actions)));
                    c.show_context_menu(ContextMenuRequest {
                        request_id: id,
                        position: *position,
                        items: labels
                            .iter()
                            .enumerate()
                            .map(|(i, label)| ContextMenuItem {
                                command_id: i as u32 + 1,
                                label: (*label).into(),
                                enabled: i != 0 || pane.view.selected_text().is_some(),
                                checked: false,
                                destructive: i >= 7,
                                separator: false,
                            })
                            .collect(),
                    });
                    return EventResult::Consumed;
                }
            }
        }
        let keyboard = matches!(
            e,
            ViewEvent::KeyPressed { .. }
                | ViewEvent::KeyReleased { .. }
                | ViewEvent::TextInput { .. }
                | ViewEvent::Backspace
        );
        let mut result = EventResult::Ignored;
        if !matches!(
            e,
            ViewEvent::KeyPressed { .. } | ViewEvent::TextInput { .. } | ViewEvent::Backspace
        ) {
            result = result.merge(self.core.tabstrip().handle_event(frames.tabs, e, c));
            let selected = self.core.tab_selected.get();
            let mut w = self.core.workspace.borrow_mut();
            if let Some(index) = w.tabs.iter().position(|t| t.id as usize == selected) {
                if w.active != index {
                    w.active = index;
                    w.zoomed = false;
                }
            }
        }
        if let Some(find) = frames
            .find
            .filter(|_| !keyboard || self.core.find.state.is_focused())
        {
            result = result.merge(Toolbar::new(self.core.finder()).handle_event(find, e, c));
        }
        let active = self.core.selected().map(|p| p.id);
        for (pane, frame) in pane_frames {
            pane.view
                .focused
                .set(active == Some(pane.id) && !self.core.find.state.is_focused());
            let pointer = e.position().is_some();
            if active == Some(pane.id)
                || pointer
                || matches!(e, ViewEvent::PointerLeft | ViewEvent::FocusChanged { .. })
            {
                result = result.merge(
                    Surface::pane()
                        .in_container(b)
                        .content(pane.view.clone())
                        .handle_event(Self::terminal_bounds(frame, split, c.theme()), e, c),
                );
            }
            if let Some(notice) = pane.view.notice.borrow_mut().take() {
                *self.core.notice.borrow_mut() = notice;
            }
        }
        if matches!(
            e,
            ViewEvent::TextInput { .. } | ViewEvent::KeyPressed { .. } | ViewEvent::Backspace
        ) {
            self.core.last_activity.set(Instant::now());
        }
        self.core.apply();
        if self.core.open_menu.replace(false) {
            let count = self.core.workspace.borrow().tabs.len();
            let x = (frames.tabs.origin.x
                + c.theme().layout.navigation_sidebar_width * count as f32
                + c.theme().controls.small_height)
                .min(
                    frames.tabs.origin.x + frames.tabs.size.width - c.theme().controls.small_height,
                );
            self.core.show_dropdown(
                Point::new(
                    x.max(frames.tabs.origin.x),
                    frames.tabs.origin.y + frames.tabs.size.height,
                ),
                c,
            );
        }
        c.request_redraw_in(b);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewkit::{draw_command::DisplayList, typography::TextMeasurer};
    fn fixture() -> TerminalUi {
        let core = Core::new();
        *core.workspace.borrow_mut() = Workspace::new(Settings::default());
        for p in core.workspace.borrow().all() {
            p.session.borrow_mut().suppress_launch();
        }
        TerminalUi { core }
    }
    #[test]
    fn workspace_commands_settings_cancel_and_close_confirmation_are_functional() {
        let v = fixture();
        let c = &v.core;
        c.queue(Action::Split(true));
        c.apply();
        assert_eq!(c.workspace.borrow().all().len(), 2);
        for p in c.workspace.borrow().all() {
            p.session.borrow_mut().suppress_launch();
        }
        c.queue(Action::ClosePane);
        c.apply();
        assert!(c.panel.get() == Panel::ClosePane);
        c.queue(Action::Cancel);
        c.apply();
        assert_eq!(c.workspace.borrow().all().len(), 2);
        c.queue(Action::ClosePane);
        c.apply();
        c.queue(Action::Confirm);
        c.apply();
        assert_eq!(c.workspace.borrow().all().len(), 1);
        c.queue(Action::New(0));
        c.apply();
        assert_eq!(c.workspace.borrow().tabs.len(), 2);
        c.queue(Action::NextTab(-1));
        c.apply();
        assert_eq!(c.workspace.borrow().active, 0);
        c.queue(Action::Settings);
        c.apply();
        c.editor
            .borrow()
            .as_ref()
            .unwrap()
            .name
            .state
            .set_value("Changed");
        c.queue(Action::Cancel);
        c.apply();
        assert_eq!(
            c.workspace.borrow().settings.borrow().profiles[0].name,
            "Shell"
        );
    }
    #[test]
    fn chrome_keyboard_does_not_consume_shell_arrows_or_plain_ctrl_c() {
        let v = fixture();
        let b = Rect::new(0., 0., 1040., 700.);
        let t = Theme::LIGHT;
        let mut text = TextMeasurer::new();
        let mut cx = EventContext::new(&t, &t.typography, &mut text);
        assert!(
            v.key(
                Key::Character('c'),
                KeyModifiers::from_bits(KeyModifiers::CONTROL)
            )
            .is_none()
        );
        v.handle_event(
            b,
            &ViewEvent::KeyPressed {
                key: Key::ArrowRight,
                modifiers: KeyModifiers::default(),
            },
            &mut cx,
        );
        assert_eq!(v.core.workspace.borrow().tabs.len(), 1);
        v.handle_event(
            b,
            &ViewEvent::KeyPressed {
                key: Key::Character('f'),
                modifiers: KeyModifiers::from_bits(KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            },
            &mut cx,
        );
        assert!(v.core.find_visible.get());
        v.handle_event(
            b,
            &ViewEvent::TextInput {
                text: "search".into(),
            },
            &mut cx,
        );
        assert_eq!(v.core.find.state.value(), "search");
        v.handle_event(
            b,
            &ViewEvent::KeyPressed {
                key: Key::Escape,
                modifiers: KeyModifiers::default(),
            },
            &mut cx,
        );
        assert!(!v.core.find_visible.get());
    }
    #[test]
    fn multiline_paste_waits_for_confirmation_and_is_bound_to_its_original_pane() {
        let v = fixture();
        let c = &v.core;
        let id = c.selected().unwrap().id;
        c.prepare_paste("echo one\necho two".into()).unwrap();
        assert!(c.panel.get() == Panel::Paste);
        assert_eq!(c.pending_paste.borrow().as_ref().unwrap().0, id);
        c.queue(Action::Cancel);
        c.apply();
        assert!(c.pending_paste.borrow().is_none());
        assert!(c.prepare_paste("x".repeat(60001)).is_err());
    }
    #[test]
    fn command_palette_keyboard_selection_runs_the_selected_command() {
        let v = fixture();
        let b = Rect::new(0., 0., 1040., 700.);
        let t = Theme::LIGHT;
        let mut text = TextMeasurer::new();
        let mut cx = EventContext::new(&t, &t.typography, &mut text);
        v.core.queue(Action::Commands);
        v.core.apply();
        v.handle_event(
            b,
            &ViewEvent::KeyPressed {
                key: Key::ArrowDown,
                modifiers: KeyModifiers::default(),
            },
            &mut cx,
        );
        assert_eq!(v.core.palette_index.get(), 1);
        v.handle_event(
            b,
            &ViewEvent::KeyPressed {
                key: Key::Enter,
                modifiers: KeyModifiers::default(),
            },
            &mut cx,
        );
        assert_eq!(v.core.workspace.borrow().all().len(), 2);
    }
    #[test]
    fn compact_bar_adds_tabs_and_opens_the_actual_action_menu() {
        let v = fixture();
        let b = Rect::new(0., 0., 1040., 700.);
        let t = Theme::LIGHT;
        let mut list = DisplayList::new();
        let mut text = TextMeasurer::new();
        let mut nodes = vec![];
        v.paint(
            b,
            &mut PaintContext::new(&mut list, &t, &t.typography, &mut text)
                .with_accessibility_nodes(&mut nodes),
        );
        assert!(
            !nodes
                .iter()
                .any(|n| n.label.as_deref() == Some("Split Right"))
        );
        let plus = nodes
            .iter()
            .find(|n| n.label.as_deref() == Some("New tab (Ctrl+Shift+T)"))
            .unwrap()
            .bounds;
        let mut event_text = TextMeasurer::new();
        let mut cx = EventContext::new(&t, &t.typography, &mut event_text);
        let click = |v: &TerminalUi, bounds: Rect, cx: &mut EventContext<'_>| {
            let p = Point::new(
                bounds.origin.x + bounds.size.width / 2.,
                bounds.origin.y + bounds.size.height / 2.,
            );
            for e in [
                ViewEvent::PointerPressed {
                    position: p,
                    button: PointerButton::Primary,
                },
                ViewEvent::PointerReleased {
                    position: p,
                    button: PointerButton::Primary,
                },
            ] {
                v.handle_event(b, &e, cx);
            }
        };
        click(&v, plus, &mut cx);
        assert_eq!(v.core.workspace.borrow().tabs.len(), 2);
        for p in v.core.workspace.borrow().all() {
            p.session.borrow_mut().suppress_launch();
        }
        nodes.clear();
        v.paint(
            b,
            &mut PaintContext::new(&mut list, &t, &t.typography, &mut text)
                .with_accessibility_nodes(&mut nodes),
        );
        let menu = nodes
            .iter()
            .find(|n| n.label.as_deref() == Some("Profiles and terminal actions"))
            .unwrap()
            .bounds;
        click(&v, menu, &mut cx);
        let request = cx
            .take_context_menu_request()
            .expect("chevron must open a real menu");
        let settings = request
            .items
            .iter()
            .find(|item| item.label == "Settings…")
            .unwrap()
            .command_id;
        v.handle_event(
            b,
            &ViewEvent::ContextMenuResult {
                request_id: request.request_id,
                command_id: Some(settings),
            },
            &mut cx,
        );
        assert!(v.core.panel.get() == Panel::Settings);
    }
    #[test]
    fn closing_an_inactive_tab_does_not_switch_the_active_session() {
        let v = fixture();
        let first = v.core.selected().unwrap().id;
        v.core.queue(Action::New(0));
        v.core.apply();
        let second = v.core.selected().unwrap().id;
        v.core.queue(Action::NextTab(-1));
        v.core.apply();
        v.core
            .workspace
            .borrow()
            .settings
            .borrow_mut()
            .confirm_close = false;
        v.core.queue(Action::CloseTabId(second));
        v.core.apply();
        assert_eq!(v.core.selected().unwrap().id, first);
        assert_eq!(v.core.workspace.borrow().tabs.len(), 1);
    }
    #[test]
    fn native_views_paint_small_large_split_and_modal_without_borrow_panics() {
        let v = fixture();
        v.core.queue(Action::Split(true));
        v.core.apply();
        for p in v.core.workspace.borrow().all() {
            let mut s = p.session.borrow_mut();
            s.suppress_launch();
            s.buffer.push_message("\x1b[32mcargo test\x1b[0m 日本語");
        }
        for theme in [Theme::LIGHT, Theme::DARK] {
            for width in [360., 1040.] {
                let b = Rect::new(0., 0., width, 600.);
                for panel in [
                    Panel::None,
                    Panel::Commands,
                    Panel::ClosePane,
                    Panel::Paste,
                    Panel::Rename,
                    Panel::Settings,
                ] {
                    if panel == Panel::Settings {
                        v.core.queue(Action::Settings);
                        v.core.apply();
                    } else {
                        v.core.panel.set(panel);
                    }
                    let mut list = DisplayList::new();
                    let mut text = TextMeasurer::new();
                    v.paint(
                        b,
                        &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut text),
                    );
                }
            }
        }
    }
}
