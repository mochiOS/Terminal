//! Protected text questions live outside the untrusted VT/PTY stream.
use crate::workspace::Pane;
use mochi_user_platform as p;
use p::capability::consent::{Choice, Correlation, frontend as wire, prompt};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};
use viewkit::{
    event::{EventContext, ViewEvent},
    prelude::*,
};

struct Reference(u32);
impl Drop for Reference {
    fn drop(&mut self) {
        let _ = p::handle::close(self.0);
    }
}
struct Owner {
    pane: Rc<Pane>,
    master: Reference,
}
struct Question {
    bytes: Vec<u8>,
    correlation: Correlation,
    armed: bool,
    selected: Option<Choice>,
    scroll: ScrollState,
}
pub(crate) struct Presenter {
    endpoint: Option<u32>,
    owner: Option<Owner>,
    question: Option<Question>,
    next: Instant,
    last_error: Option<i64>,
    notice: Option<String>,
    release_after_decision: Option<Key>,
}
fn invalid() -> p::syscall::SysError {
    p::syscall::SysError::from(p::syscall::NativeError::AccessDenied.into_raw())
}
fn retryable(error: p::syscall::SysError) -> bool {
    error.errno() == Some(mochios_posix_client::newlib_errno::EAGAIN as u64)
}
impl Presenter {
    pub fn new() -> Self {
        Self {
            endpoint: None,
            owner: None,
            question: None,
            next: Instant::now(),
            last_error: None,
            notice: None,
            release_after_decision: None,
        }
    }
    fn call(
        &mut self,
        bytes: &[u8],
        output: &mut [u8],
        master: Option<u32>,
    ) -> Result<usize, p::syscall::SysError> {
        let endpoint = match self.endpoint {
            Some(endpoint) => endpoint,
            None => {
                let endpoint = u32::try_from(p::process::find_by_name("capability.service")?)
                    .map_err(|_| invalid())?;
                self.endpoint = Some(endpoint);
                endpoint
            }
        };
        // The privileged broker is in the authority TCB. This query and the
        // canonical reply sender authenticate it without process.inspect in
        // Terminal or a name/digest supplied by an application.
        if p::capability::check_thread(u64::from(endpoint), "capabilities.manage") != Ok(1) {
            self.endpoint = None;
            return Err(invalid());
        }
        let mut objects = p::ipc::IpcObjectHandles::default();
        if let Some(master) = master {
            objects.count = 1;
            objects.handles[0] = p::ipc::IpcObjectHandle {
                handle: master,
                reserved: 0,
                rights: p::handle::HANDLE_RIGHT_CONTROL | p::handle::HANDLE_RIGHT_TRANSFER,
            };
        }
        let result = p::ipc::call_object_handles(endpoint, bytes, output, &mut objects);
        let unexpected = objects.count != 0 || objects.delivery_flags != 0;
        for (i, object) in objects
            .handles
            .iter()
            .take((objects.count as usize).min(objects.handles.len()))
            .enumerate()
        {
            if object.handle != 0
                && Some(object.handle) != master
                && !objects.handles[..i]
                    .iter()
                    .any(|old| old.handle == object.handle)
            {
                let _ = p::handle::close(object.handle);
            }
        }
        let message = result?;
        if unexpected
            || message >> 32 != u64::from(endpoint)
            || message as u32 as usize > output.len()
            || p::capability::check_thread(u64::from(endpoint), "capabilities.manage") != Ok(1)
        {
            return Err(invalid());
        }
        let length = message as u32 as usize;
        if length == 8 {
            let status = u64::from_le_bytes(output[..8].try_into().unwrap());
            if status != 0 {
                return Err(if status <= 4095 {
                    p::syscall::SysError::from_raw(-(status as i64))
                } else {
                    invalid()
                });
            }
        }
        Ok(length)
    }
    fn command(
        &mut self,
        correlation: Correlation,
        action: wire::Action,
    ) -> Result<(), p::syscall::SysError> {
        let bytes = wire::encode(correlation, action).map_err(|_| invalid())?;
        let mut output = [0; 8];
        if self.call(&bytes, &mut output, None)? != 8 {
            return Err(invalid());
        }
        Ok(())
    }
    fn clear_question(&mut self) {
        self.question = None;
        if let Some(owner) = &self.owner {
            owner
                .pane
                .session
                .borrow_mut()
                .protect_input(self.release_after_decision.is_some());
        }
    }
    fn failure(&mut self, stage: &str, reason: i64) {
        if self.last_error != Some(reason) {
            let message = format!("Terminal permission {stage} failed (status {reason})");
            p::logln!("{}", message);
            self.notice = Some(message);
            self.last_error = Some(reason);
        }
        self.withdraw();
        self.next = Instant::now() + Duration::from_secs(1);
    }
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
    pub fn withdraw(&mut self) {
        self.release_after_decision = None;
        if let Some(question) = self.question.take() {
            let _ = self.command(question.correlation, wire::Action::Dismissed);
        }
        if let Some(owner) = self.owner.take() {
            let _ = self.call(
                &wire::encode_text_poll(true),
                &mut [0; 8],
                Some(owner.master.0),
            );
            owner.pane.session.borrow_mut().protect_input(false);
        }
    }
    pub fn poll(&mut self, selected: Option<Rc<Pane>>, enabled: bool) {
        let selected = selected.filter(|pane| pane.session.borrow().running());
        if !enabled
            || self.owner.as_ref().is_some_and(|owner| {
                selected
                    .as_ref()
                    .is_none_or(|pane| pane.id != owner.pane.id)
            })
        {
            self.withdraw();
        }
        if !enabled || Instant::now() < self.next {
            return;
        }
        self.next = Instant::now() + Duration::from_millis(100);
        if self.owner.is_none() {
            let Some(pane) = selected else {
                return;
            };
            let master = match pane.session.borrow().consent_master() {
                Ok(Some(master)) => Reference(master),
                Ok(None) => return,
                Err(error) => {
                    self.failure(
                        "controller export",
                        i64::from(error.raw_os_error().unwrap_or(5)),
                    );
                    return;
                }
            };
            self.owner = Some(Owner { pane, master });
        }
        let master = self.owner.as_ref().unwrap().master.0;
        let mut bytes = [0; prompt::MAX_MESSAGE_BYTES];
        match self.call(&wire::encode_text_poll(false), &mut bytes, Some(master)) {
            Ok(8) => {
                if self.last_error.take().is_some() {
                    self.notice = Some(String::new());
                }
                self.clear_question();
            }
            Ok(length) => {
                let offer = match prompt::decode(&bytes[..length]) {
                    Ok(offer) => offer,
                    Err(_) => {
                        self.withdraw();
                        return;
                    }
                };
                let correlation = offer.correlation;
                if self
                    .question
                    .as_ref()
                    .is_none_or(|question| question.correlation != correlation)
                {
                    self.question = Some(Question {
                        bytes: bytes[..length].to_vec(),
                        correlation,
                        armed: false,
                        selected: None,
                        scroll: ScrollState::new(),
                    });
                    self.owner
                        .as_ref()
                        .unwrap()
                        .pane
                        .session
                        .borrow_mut()
                        .protect_input(true);
                } else if self.question.as_ref().unwrap().bytes != bytes[..length] {
                    self.withdraw();
                    return;
                }
                match self.command(correlation, wire::Action::Presented) {
                    Ok(()) => self.question.as_mut().unwrap().armed = true,
                    Err(reason) if retryable(reason) => {}
                    Err(reason) => self.failure("presentation", reason.raw()),
                }
            }
            Err(reason) if retryable(reason) => {}
            Err(reason) => self.failure("registration", reason.raw()),
        }
    }
    pub fn binding(&self) -> Option<presentation::Protected> {
        self.question
            .as_ref()
            .map(|q| presentation::binding(q.correlation))
    }
    fn content(&self) -> Option<ContentArea<VStack>> {
        let question = self.question.as_ref()?;
        let offer = prompt::decode(&question.bytes).unwrap();
        let mut text = String::new();
        offer.write_text(&mut text).unwrap();
        let status = if !question.armed {
            "Waiting for protected display and released keys…"
        } else {
            match question.selected {
                Some(Choice::AllowOnce) => "Allow Once selected — press Enter to confirm.",
                Some(Choice::AllowPersistently) => {
                    "Always Allow selected — press Enter to confirm."
                }
                _ => "Choose y or a, then press Enter. n / Escape cancels.",
            }
        };
        Some(ContentArea::pane(
            VStack::new()
                .gap(StackGap::Large)
                .child(Text::styled(
                    "Application Permissions",
                    TextRole::TitleMedium,
                ))
                .child(
                    Scroll::new(question.scroll.clone())
                        .content(Text::styled(text, TextRole::Code).monospaced())
                        .layout()
                        .flex_grow(1.0)
                        .flex_shrink(1.0),
                )
                .child(Text::body(status)),
        ))
    }
    pub fn paint(&self, bounds: Rect, context: &mut viewkit::view::PaintContext<'_>) -> bool {
        let Some(content) = self.content() else {
            return false;
        };
        content.paint(bounds, context);
        true
    }
    pub fn event(
        &mut self,
        bounds: Rect,
        event: &ViewEvent,
        context: &mut EventContext<'_>,
    ) -> bool {
        if matches!(event, ViewEvent::FocusChanged { focused: false })
            || ((self.question.is_some() || self.release_after_decision.is_some())
                && matches!(event, ViewEvent::PointerPressed { position, .. } if !bounds.contains(*position)))
        {
            self.withdraw();
            return false;
        }
        if let Some(decision_key) = self.release_after_decision {
            if matches!(event, ViewEvent::KeyReleased { key, .. } if *key == decision_key) {
                self.release_after_decision = None;
                if let Some(owner) = &self.owner {
                    owner
                        .pane
                        .session
                        .borrow_mut()
                        .protect_input(self.question.is_some());
                }
            }
            if matches!(
                event,
                ViewEvent::KeyPressed { .. }
                    | ViewEvent::KeyReleased { .. }
                    | ViewEvent::TextInput { .. }
            ) {
                return true;
            }
        }
        if self.question.is_none() {
            return false;
        }
        if let Some(content) = self.content() {
            content.handle_event(bounds, event, context);
        }
        if let ViewEvent::KeyPressed { key, modifiers } = event {
            if *key == Key::Tab && modifiers.shortcut() {
                self.withdraw();
                return false;
            }
            let correlation = self.question.as_ref().unwrap().correlation;
            let plain = !modifiers.shift()
                && !modifiers.control()
                && !modifiers.alt()
                && !modifiers.super_key();
            let proposed = if plain {
                match key {
                    Key::Escape => Some(Choice::Deny),
                    Key::Character(c) if c.eq_ignore_ascii_case(&'n') => Some(Choice::Deny),
                    Key::Enter if self.question.as_ref().unwrap().armed => {
                        self.question.as_ref().unwrap().selected
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some(choice) = proposed {
                match self.command(correlation, wire::Action::Decide(choice)) {
                    Ok(()) => {
                        self.release_after_decision = Some(*key);
                        self.clear_question();
                    }
                    Err(reason) if retryable(reason) => {
                        self.question.as_mut().unwrap().selected = None
                    }
                    Err(_) => self.withdraw(),
                }
            } else {
                let question = self.question.as_mut().unwrap();
                question.selected = if plain && question.armed {
                    match key {
                        Key::Character(c) if c.is_ascii() => prompt::decode(&question.bytes)
                            .unwrap()
                            .choice_for_shortcut(*c as u8)
                            .filter(|choice| {
                                matches!(choice, Choice::AllowOnce | Choice::AllowPersistently)
                            }),
                        _ => None,
                    }
                } else {
                    None
                };
            }
        }
        // TextInput (including IME and paste), queued VT bytes and menu input
        // cannot become a choice. Actual keys are independently checked again.
        true
    }
}
impl Drop for Presenter {
    fn drop(&mut self) {
        self.withdraw();
    }
}
mod presentation {
    pub type Protected = viewkit::platform::ProtectedPresentation;
    pub fn binding(c: mochi_user_platform::capability::consent::Correlation) -> Protected {
        Protected {
            ticket: c.ticket.id,
            challenge: c.ticket.challenge,
            display: c.display,
        }
    }
}
