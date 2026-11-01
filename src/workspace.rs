//! Session ownership and split geometry, independent of application chrome.
use crate::{
    session::TerminalSession,
    settings::{Profile, Settings},
    terminal_view::TerminalView,
};
use std::{cell::RefCell, rc::Rc};
use viewkit::geometry::Rect;
pub(crate) const MAX_SESSIONS: usize = 16;
pub(crate) struct Pane {
    pub id: u64,
    pub session: Rc<RefCell<TerminalSession>>,
    pub view: Rc<TerminalView>,
}
pub(crate) enum Tree {
    Leaf(Rc<Pane>),
    Split {
        horizontal: bool,
        ratio: f32,
        first: Box<Tree>,
        second: Box<Tree>,
    },
}
impl Tree {
    pub fn leaves(&self, out: &mut Vec<Rc<Pane>>) {
        match self {
            Self::Leaf(p) => out.push(p.clone()),
            Self::Split { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
        }
    }
    pub fn frames(&self, b: Rect, gap: f32, out: &mut Vec<(Rc<Pane>, Rect)>) {
        match self {
            Self::Leaf(p) => out.push((p.clone(), b)),
            Self::Split {
                horizontal,
                ratio,
                first,
                second,
            } => {
                let extent = if *horizontal {
                    b.size.width
                } else {
                    b.size.height
                };
                let gap = gap.min(extent);
                let first_size = (extent - gap) * ratio;
                let second_size = (extent - gap - first_size).max(0.0);
                let a = if *horizontal {
                    Rect::new(b.origin.x, b.origin.y, first_size, b.size.height)
                } else {
                    Rect::new(b.origin.x, b.origin.y, b.size.width, first_size)
                };
                let c = if *horizontal {
                    Rect::new(
                        b.origin.x + first_size + gap,
                        b.origin.y,
                        second_size,
                        b.size.height,
                    )
                } else {
                    Rect::new(
                        b.origin.x,
                        b.origin.y + first_size + gap,
                        b.size.width,
                        second_size,
                    )
                };
                first.frames(a, gap, out);
                second.frames(c, gap, out);
            }
        }
    }
    fn split(&mut self, id: u64, pane: Rc<Pane>, horizontal: bool) -> bool {
        match self {
            Self::Leaf(p) if p.id == id => {
                *self = Self::Split {
                    horizontal,
                    ratio: 0.5,
                    first: Box::new(Self::Leaf(p.clone())),
                    second: Box::new(Self::Leaf(pane)),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.split(id, pane.clone(), horizontal) || second.split(id, pane, horizontal)
            }
            _ => false,
        }
    }
    fn remove(self, id: u64) -> Option<Self> {
        match self {
            Self::Leaf(p) => {
                if p.id == id {
                    None
                } else {
                    Some(Self::Leaf(p))
                }
            }
            Self::Split {
                horizontal,
                ratio,
                first,
                second,
            } => match (first.remove(id), second.remove(id)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    horizontal,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (a, b) => a.or(b),
            },
        }
    }
    pub fn resize_split(&mut self, path: &[bool], ratio: f32) {
        if let Self::Split {
            ratio: value,
            first,
            second,
            ..
        } = self
        {
            if let Some((right, rest)) = path.split_first() {
                if *right {
                    second.resize_split(rest, ratio)
                } else {
                    first.resize_split(rest, ratio)
                }
            } else {
                *value = ratio.clamp(0.15, 0.85);
            }
        }
    }
    pub fn dividers(
        &self,
        b: Rect,
        gap: f32,
        path: Vec<bool>,
        out: &mut Vec<(Vec<bool>, Rect, bool, Rect)>,
    ) {
        if let Self::Split {
            horizontal,
            ratio,
            first,
            second,
        } = self
        {
            let extent = if *horizontal {
                b.size.width
            } else {
                b.size.height
            };
            let gap = gap.min(extent.max(0.0));
            let position = (extent - gap).max(0.0) * ratio;
            let divider = if *horizontal {
                Rect::new(b.origin.x + position, b.origin.y, gap, b.size.height)
            } else {
                Rect::new(b.origin.x, b.origin.y + position, b.size.width, gap)
            };
            out.push((path.clone(), divider, *horizontal, b));
            let a = if *horizontal {
                Rect::new(b.origin.x, b.origin.y, position, b.size.height)
            } else {
                Rect::new(b.origin.x, b.origin.y, b.size.width, position)
            };
            let c = if *horizontal {
                Rect::new(
                    divider.origin.x + gap,
                    b.origin.y,
                    (extent - position - gap).max(0.0),
                    b.size.height,
                )
            } else {
                Rect::new(
                    b.origin.x,
                    divider.origin.y + gap,
                    b.size.width,
                    (extent - position - gap).max(0.0),
                )
            };
            let mut left = path.clone();
            left.push(false);
            let mut right = path;
            right.push(true);
            first.dividers(a, gap, left, out);
            second.dividers(c, gap, right, out);
        }
    }
}
pub(crate) struct Tab {
    pub id: u64,
    pub root: Option<Tree>,
    pub active: u64,
    pub name: Option<String>,
}
pub(crate) struct Workspace {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub zoomed: bool,
    next: u64,
    pub settings: Rc<RefCell<Settings>>,
}
impl Workspace {
    pub fn new(settings: Settings) -> Self {
        let mut workspace = Self {
            tabs: vec![],
            active: 0,
            zoomed: false,
            next: 1,
            settings: Rc::new(RefCell::new(settings)),
        };
        let profile = workspace.settings.borrow().default_profile;
        let _ = workspace.new_tab(profile);
        workspace
    }
    fn pane(&mut self, profile: Profile) -> Rc<Pane> {
        let id = self.next;
        self.next += 1;
        let session = Rc::new(RefCell::new(TerminalSession::with_profile(
            profile,
            self.settings.borrow().scrollback,
        )));
        let view = Rc::new(TerminalView::new(session.clone(), self.settings.clone()));
        Rc::new(Pane { id, session, view })
    }
    pub fn all(&self) -> Vec<Rc<Pane>> {
        let mut out = vec![];
        for tab in &self.tabs {
            if let Some(root) = &tab.root {
                root.leaves(&mut out);
            }
        }
        out
    }
    pub fn selected(&self) -> Option<Rc<Pane>> {
        let id = self.tabs.get(self.active)?.active;
        self.all().into_iter().find(|pane| pane.id == id)
    }
    pub fn new_tab(&mut self, profile: usize) -> Result<(), String> {
        if self.all().len() >= MAX_SESSIONS || self.tabs.len() >= 12 {
            return Err(
                "Close a session before opening another (16 sessions / 12 tabs maximum).".into(),
            );
        }
        let profile = self
            .settings
            .borrow()
            .profiles
            .get(profile)
            .cloned()
            .ok_or("Profile no longer exists")?;
        profile.validate()?;
        let pane = self.pane(profile);
        let id = pane.id;
        self.tabs.push(Tab {
            id,
            root: Some(Tree::Leaf(pane)),
            active: id,
            name: None,
        });
        self.active = self.tabs.len() - 1;
        self.zoomed = false;
        Ok(())
    }
    pub fn split(&mut self, horizontal: bool) -> Result<(), String> {
        let tab = self.tabs.get(self.active).ok_or("Open a tab first")?;
        let mut leaves = vec![];
        if let Some(root) = &tab.root {
            root.leaves(&mut leaves);
        }
        if leaves.len() >= 8 || self.all().len() >= MAX_SESSIONS {
            return Err("This workspace has reached its pane limit.".into());
        }
        let profile = self
            .selected()
            .ok_or("Select a pane first")?
            .session
            .borrow()
            .profile
            .clone();
        let pane = self.pane(profile);
        let id = pane.id;
        let tab = &mut self.tabs[self.active];
        if let Some(root) = &mut tab.root {
            root.split(tab.active, pane, horizontal);
        }
        tab.active = id;
        self.zoomed = false;
        Ok(())
    }
    pub fn close_pane(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if let Some(root) = tab.root.take() {
                tab.root = root.remove(tab.active);
            }
            if tab.root.is_none() {
                self.tabs.remove(self.active);
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
            } else {
                let mut leaves = vec![];
                tab.root.as_ref().unwrap().leaves(&mut leaves);
                tab.active = leaves[0].id;
            }
        }
        self.zoomed = false;
    }
    pub fn close_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
        self.zoomed = false;
    }
    pub fn close_tab_id(&mut self, id: u64) {
        let active = self.tabs.get(self.active).map(|t| t.id);
        if let Some(index) = self.tabs.iter().position(|t| t.id == id) {
            self.tabs.remove(index);
            self.active = active
                .and_then(|id| self.tabs.iter().position(|t| t.id == id))
                .unwrap_or(index.min(self.tabs.len().saturating_sub(1)));
            self.zoomed = false;
        }
    }
    pub fn focus_next(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let mut panes = vec![];
            if let Some(root) = &tab.root {
                root.leaves(&mut panes);
            }
            if let Some(index) = panes.iter().position(|p| p.id == tab.active) {
                tab.active = panes[(index + 1) % panes.len()].id;
            }
        }
    }
    pub fn frames(&self, b: Rect, gap: f32) -> Vec<(Rc<Pane>, Rect)> {
        let mut out = vec![];
        if self.zoomed {
            if let Some(p) = self.selected() {
                out.push((p, b));
            }
        } else if let Some(root) = self.tabs.get(self.active).and_then(|tab| tab.root.as_ref()) {
            root.frames(b, gap, &mut out);
        }
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tabs_and_split_sessions_have_independent_ownership_and_collapse() {
        let mut w = Workspace::new(Settings::default());
        let first = w.selected().unwrap().id;
        w.split(true).unwrap();
        assert_ne!(first, w.selected().unwrap().id);
        w.split(false).unwrap();
        assert_eq!(w.all().len(), 3);
        let b = Rect::new(0.0, 0.0, 800.0, 600.0);
        let frames = w.frames(b, 4.0);
        for (i, (_, a)) in frames.iter().enumerate() {
            for (_, c) in frames.iter().skip(i + 1) {
                assert!(a.intersection(*c).is_none());
            }
        }
        w.close_pane();
        assert_eq!(w.all().len(), 2);
        w.focus_next();
        w.close_pane();
        assert_eq!(w.selected().unwrap().id, first);
        w.new_tab(0).unwrap();
        assert_eq!(w.tabs.len(), 2);
        w.close_tab();
        assert_eq!(w.tabs.len(), 1);
    }
}
