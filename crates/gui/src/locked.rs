//! Locked controllers: the pads that hide their config port until they get an unlock
//! sequence, what the window knows about each, and the controllers it can relock.

use std::sync::mpsc::Sender;

use controller_core::device::{Model, UsbId};
use controller_core::transport::lock::Unlocked;
use controller_core::{devices, Error};
use tr::tr;

use crate::holders;
use crate::state::AppState;
use crate::ui::{AppWindow, CloseInfo, LockedInfo, Relock};
use crate::worker::{Command, Failure, Target};
use slint::ComponentHandle as _;

/// The guide to checking a controller on a real pad, which the refusal screen links to.
pub const GUIDE_URL: &str =
    concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/master/docs/validating-a-controller.md");

/// Where a locked controller stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockStatus {
    /// Waiting for the user to unlock it.
    Locked,
    /// The unlock is queued or running.
    Unlocking,
    /// The pad did not unlock.
    Failed {
        /// The programs with the pad's USB node open, if any.
        holders: Vec<String>,
        /// The unlock was denied by the system: the access screen asks for the rule.
        denied: bool,
        /// The pad did not answer in time: a replug may clear it.
        timed_out: bool,
    },
    /// The unlock brought up a USB id the app does not use. Nothing more is sent;
    /// replugging returns the pad to `XInput`. The name is the display name of the
    /// unvalidated model the id names, or `None` when no description lists the id.
    Unsupported(UsbId, Option<String>),
    /// The pad showed its config port with no unlock, and its model is not validated.
    /// Nothing was sent, and it never locked: no replug advice. Holds the model's name.
    OnPort(UsbId, String),
}

/// A locked controller, listed in the picker as "locked".
#[derive(Debug, Clone)]
pub struct LockedPad {
    /// The USB port path.
    pub port: String,
    /// The model whose lock declaration matched.
    pub model: &'static dyn Model,
    /// Where it stands.
    pub status: LockStatus,
}

impl LockedPad {
    /// The pad's model is not validated, and the user did not let it through: the panel
    /// refuses it, and no unlock is sent.
    #[must_use]
    pub fn refused(&self) -> bool {
        self.model.description().is_ok_and(devices::refused)
    }

    /// The unlock failed for a reason the panel explains (not a denied one, which the
    /// access screen takes).
    #[must_use]
    pub const fn failed(&self) -> bool {
        matches!(self.status, LockStatus::Failed { denied: false, .. })
    }
}

impl AppState {
    /// The worker listed every locked controller attached. A pad keeps its status while
    /// it stays listed, except an unsupported one that is listed again: it returned to
    /// its locked id, so it starts over. A new pad starts locked; one that left is
    /// dropped, except an unsupported one, which stays until [`Self::unplugged`].
    pub fn locked_seen(&mut self, seen: Vec<Target>) {
        let old = std::mem::take(&mut self.locked);
        self.locked = seen
            .into_iter()
            .map(|t| {
                let status =
                    old.iter().find(|p| p.port == t.port).map(|p| p.status.clone()).filter(|s| {
                        !matches!(s, LockStatus::Unsupported(..) | LockStatus::OnPort(..))
                    });
                LockedPad {
                    port: t.port,
                    model: t.model,
                    status: status.unwrap_or(LockStatus::Locked),
                }
            })
            .collect();
        let kept: Vec<_> = old
            .into_iter()
            .filter(|p| {
                matches!(p.status, LockStatus::Unsupported(..) | LockStatus::OnPort(..))
                    && !self.locked.iter().any(|q| q.port == p.port)
            })
            .collect();
        self.locked.extend(kept);
    }

    /// A pad of a refused model showed its config port: it is listed as a locked pad
    /// with the refusal, and nothing is read from it.
    pub fn refused_seen(&mut self, target: Target, usb: UsbId) {
        let name = target.model.description().ok().map(|d| d.display_name.clone());
        let status = LockStatus::OnPort(usb, name.unwrap_or_default());
        if self.locked.iter().any(|p| p.port == target.port) {
            self.set_lock_status(&target.port, status);
        } else {
            self.locked.push(LockedPad { port: target.port, model: target.model, status });
        }
    }

    /// An unsupported pad on `port` left the bus: its message goes.
    pub fn unplugged(&mut self, port: &str) {
        self.locked.retain(|p| p.port != port);
    }

    /// The user asked to unlock the pad on `port`. A pad already unlocking, or
    /// unsupported, is left alone. Returns whether the command went out.
    pub fn unlock_requested(&mut self, commands: &Sender<Command>, port: &str) -> bool {
        let Some(pad) = self.locked.iter().find(|p| p.port == port) else { return false };
        if pad.refused()
            || matches!(
                pad.status,
                LockStatus::Unlocking | LockStatus::Unsupported(..) | LockStatus::OnPort(..)
            )
        {
            return false;
        }
        if !self.send(commands, Command::Unlock(port.to_owned())) {
            return false;
        }
        self.set_lock_status(port, LockStatus::Unlocking);
        true
    }

    /// The unlock of the pad on `port` ended. A pad that unlocked leaves the list; the
    /// caller then treats its config port as present.
    pub fn unlock_finished(&mut self, port: &str, result: Result<&Unlocked, &Failure>) {
        match result {
            Ok(Unlocked::Port(_)) => {
                // The controller shows once its read is back, if its panel showed: a
                // user who picked another controller meanwhile keeps it.
                if self.shown_lock().is_some_and(|p| p.port == port) {
                    self.locked_shown = None;
                    self.active_port = Some(port.to_owned());
                }
                self.locked.retain(|p| p.port != port);
            }
            Ok(Unlocked::Unsupported(usb, model)) => {
                self.set_lock_status(port, LockStatus::Unsupported(*usb, model.clone()));
            }
            Err(f) => self.set_lock_status(
                port,
                LockStatus::Failed {
                    holders: f.holders.clone(),
                    denied: matches!(f.error, Error::PermissionDenied(_)),
                    timed_out: matches!(f.error, Error::Timeout),
                },
            ),
        }
    }

    fn set_lock_status(&mut self, port: &str, status: LockStatus) {
        if let Some(p) = self.locked.iter_mut().find(|p| p.port == port) {
            p.status = status;
        }
    }

    /// The listed controllers whose description declares a lock, which Relock applies
    /// to: the menu entry shows for these, and a close relocks them.
    #[must_use]
    pub fn relock_targets(&self) -> Vec<Target> {
        self.listed()
            .filter(|c| c.mode.is_some())
            .filter_map(|c| {
                let description = c.description.filter(|d| d.lock.is_some())?;
                let model = devices::models()
                    .iter()
                    .copied()
                    .find(|m| m.description().is_ok_and(|d| std::ptr::eq(d, description)))?;
                Some(Target { port: c.port.clone(), model })
            })
            .collect()
    }

    /// The locked pad the window shows, if it shows one: the one picked in the header,
    /// else the first while no controller is listed.
    #[must_use]
    pub fn shown_lock(&self) -> Option<&LockedPad> {
        let picked = self.locked_shown.as_deref();
        self.locked
            .iter()
            .find(|p| Some(p.port.as_str()) == picked)
            .or_else(|| self.listed().next().is_none().then(|| self.locked.first()).flatten())
    }

    /// Whether the window shows an editor or a locked panel, not the connect screen.
    #[must_use]
    pub fn shows_pad(&self) -> bool {
        self.has_controller() || !self.locked.is_empty()
    }

    /// The header's entries: each listed controller, then each locked pad.
    #[must_use]
    pub fn picker_labels(&self) -> Vec<String> {
        let mut labels = self.controller_labels();
        let base: Vec<String> = self
            .locked
            .iter()
            .map(|p| {
                match &p.status {
                    LockStatus::Unsupported(_, Some(name)) | LockStatus::OnPort(_, name) => {
                        return tr!("{name} · Not tested", name = name);
                    }
                    LockStatus::Unsupported(_, None) => return tr!("8BitDo · Not supported"),
                    _ => {}
                }
                let name = p.model.description().map_or("", |d| d.family());
                tr!("{name} · Locked", name = name)
            })
            .collect();
        for (i, label) in base.iter().enumerate() {
            let same = base.iter().filter(|b| *b == label).count();
            let nth = base.iter().take(i + 1).filter(|b| *b == label).count();
            labels.push(if same > 1 {
                tr!("{label} ({n})", label = label, n = nth)
            } else {
                label.clone()
            });
        }
        labels
    }

    /// The entry of [`Self::picker_labels`] the window shows.
    #[must_use]
    pub fn picker_index(&self) -> usize {
        self.shown_lock().map_or_else(
            || self.active_index(),
            |lock| {
                let at = self.locked.iter().position(|p| p.port == lock.port).unwrap_or(0);
                self.listed().count() + at
            },
        )
    }

    /// Shows entry `index` of [`Self::picker_labels`].
    pub fn pick_entry(&mut self, index: usize) {
        let listed = self.listed().count();
        if index < listed {
            self.locked_shown = None;
            self.pick_controller(index);
        } else if let Some(pad) = self.locked.get(index - listed) {
            self.locked_shown = Some(pad.port.clone());
            self.notice = None;
            self.drop_waiting_batch();
        }
    }

    /// "Check again" on the access screen: every pad whose unlock was denied is
    /// unlocked once more. Returns whether any was, which also clears the screen until
    /// an answer comes back.
    pub fn recheck_locks(&mut self, commands: &Sender<Command>) -> bool {
        let denied: Vec<String> = self
            .locked
            .iter()
            .filter(|p| matches!(p.status, LockStatus::Failed { denied: true, .. }))
            .map(|p| p.port.clone())
            .collect();
        for port in &denied {
            self.set_lock_status(port, LockStatus::Locked);
            self.unlock_requested(commands, port);
        }
        if !denied.is_empty() {
            self.access = None;
        }
        !denied.is_empty()
    }

    /// Whether the ☰ menu offers Relock: the shown controller is unlocked and its
    /// description declares a lock.
    #[must_use]
    pub fn relock_available(&self) -> bool {
        self.relock_sent.is_none()
            && self.active().is_some_and(|a| self.relock_targets().iter().any(|t| t.port == a.port))
    }

    /// Whether the shown controller has edits a relock would lose.
    #[must_use]
    pub fn relock_loses_edits(&self) -> bool {
        self.active().is_some_and(super::controllers::Controller::has_edits)
    }

    /// "Relock" was chosen. With edits it opens the question; without, it relocks now.
    pub fn relock_clicked(&mut self, commands: &Sender<Command>) {
        if !self.relock_available() || self.write.running.is_some() {
            return;
        }
        if self.relock_loses_edits() {
            self.relock_asking = true;
        } else {
            self.relock_now(commands);
        }
    }

    /// Sends a relock of the shown controller.
    fn relock_now(&mut self, commands: &Sender<Command>) {
        self.relock_asking = false;
        let Some(port) = self.active().map(|c| c.port.clone()) else { return };
        self.relock_target_sent(commands, &port);
    }

    /// Sends the relock of the controller on `port`, once at a time.
    pub fn relock_target_sent(&mut self, commands: &Sender<Command>, port: &str) {
        if self.relock_sent.is_some() {
            return;
        }
        if let Some(target) = self.relock_targets().into_iter().find(|t| t.port == port) {
            if self.send(commands, Command::Relock(target)) {
                self.relock_sent = Some(port.to_owned());
            }
        }
    }

    /// The edits went while the relock question was open, such as after a read: it
    /// closes, so it cannot come back with the next edit.
    pub fn settle_relock(&mut self) {
        if self.relock_asking && !self.relock_loses_edits() {
            self.relock_asking = false;
        }
    }

    /// Cancel on the question.
    pub const fn relock_cancelled(&mut self) {
        self.relock_asking = false;
    }

    /// "Discard and relock": the edits go with the relock.
    pub fn relock_discarded(&mut self, commands: &Sender<Command>) {
        if self.relock_asking {
            self.relock_now(commands);
        }
    }

    /// "Write n slots…" on the question: the shown controller's edited slots are
    /// written, and the pad relocks once every write worked. Returns the controller to
    /// read again before the review opens.
    pub fn relock_write(&mut self) -> Option<String> {
        if !self.relock_asking || !self.can_write_before_close() {
            return None;
        }
        self.relock_asking = false;
        let port = self.begin_batch()?;
        self.relock_after_write = Some(port.clone());
        Some(port)
    }

    /// After a write came back: the relock the question chained, if the write worked
    /// all the way and the chain still stands.
    pub const fn take_chained_relock(&mut self) -> Option<String> {
        if self.write.batch.is_some() {
            return None;
        }
        self.relock_after_write.take()
    }

    /// The question the relock asks, as the close question shows its rows.
    #[must_use]
    pub fn relock_info(&self) -> CloseInfo {
        let Some(c) = self.active().filter(|_| self.relock_asking) else {
            return CloseInfo::default();
        };
        let (group, total) = self.close_group("", c);
        let write = if self.can_write_before_close() { total } else { 0 };
        CloseInfo {
            shown: total > 0,
            total: crate::review::count(total),
            write: crate::review::count(write),
            groups: slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(vec![group]))),
        }
    }

    /// The relock of `port` worked: the pad is locked, its edits went with it, and its
    /// locked panel shows, unless another controller was picked meanwhile.
    pub fn relocked(&mut self, port: &str) {
        self.relock_asking = false;
        self.relock_sent = None;
        let shown = self.active().is_some_and(|c| c.port == port);
        let target = self.relock_targets().into_iter().find(|t| t.port == port);
        if let Some(c) = self.controller_mut(port) {
            for slot in c.slots.values_mut() {
                slot.edited = None;
                slot.edited_macros = None;
            }
        }
        self.presence(port, None);
        // Listed now, not at the worker's next poll, so the window never shows the
        // connect screen between the two.
        if let Some(t) = target.filter(|t| !self.locked.iter().any(|p| p.port == t.port)) {
            self.locked.push(LockedPad {
                port: t.port,
                model: t.model,
                status: LockStatus::Locked,
            });
        }
        if shown {
            self.locked_shown = Some(port.to_owned());
        }
    }

    /// The relock of `port` failed.
    pub fn relock_failed(&mut self, failure: &Failure) {
        self.relock_asking = false;
        self.relock_sent = None;
        self.relock_after_write = None;
        self.notice = Some(crate::state::Notice {
            error: true,
            title: tr!("The controller did not relock."),
            body: crate::plain::read_failure(&failure.error),
        });
    }

    /// The line under the controller name for the shown locked pad.
    #[must_use]
    pub fn lock_status(&self) -> Option<String> {
        let pad = self.shown_lock()?;
        Some(match pad.status {
            _ if pad.refused() => tr!("Not tested yet"),
            LockStatus::Unsupported(_, Some(_)) | LockStatus::OnPort(..) => tr!("Not tested yet"),
            LockStatus::Locked => tr!("Locked"),
            LockStatus::Unlocking => tr!("Unlocking…"),
            LockStatus::Failed { denied: true, .. } => tr!("Locked"),
            LockStatus::Failed { .. } => tr!("Did not unlock"),
            LockStatus::Unsupported(..) => tr!("Not supported yet"),
        })
    }

    /// What the locked panel shows, `view` 0 when it does not.
    #[must_use]
    pub fn locked_info(&self) -> LockedInfo {
        let Some(pad) = self.shown_lock() else { return LockedInfo::default() };
        let description = pad.model.description().ok();
        let name = description.map_or("", |d| d.family());
        let refused = pad.refused();
        let (view, bar_body) = match &pad.status {
            _ if refused => (4, String::new()),
            LockStatus::Locked | LockStatus::Failed { denied: true, .. } => (1, String::new()),
            LockStatus::Unlocking => (2, String::new()),
            LockStatus::Failed { holders, timed_out, .. } => {
                let mut body = holders::hint(holders, self.sandboxed).unwrap_or_default();
                if *timed_out {
                    if !body.is_empty() {
                        body.push(' ');
                    }
                    body.push_str(&tr!("Replug it if this repeats."));
                }
                (3, body)
            }
            LockStatus::Unsupported(..) | LockStatus::OnPort(..) => (4, String::new()),
        };
        let usb_id = |usb: &UsbId| format!("{:04x}:{:04x}", usb.vendor, usb.product);
        let (title, body) = match (&pad.status, description) {
            // The pad showed its config port: it never locked.
            (LockStatus::OnPort(usb, model), _) => (
                tr!("{model} is not tested yet", model = model),
                tr!("USB ID {pid}.", pid = usb_id(usb)),
            ),
            // The unlocked id names a model no one has checked.
            (LockStatus::Unsupported(usb, Some(model)), _) => (
                tr!("{model} is not tested yet", model = model),
                tr!("USB ID {pid}. Replug it to get XInput back.", pid = usb_id(usb)),
            ),
            // The product string names a model no one has checked: nothing was sent.
            (_, Some(d)) if refused => (
                tr!("This {family} is not tested yet", family = name),
                tr!(
                    "It reports as {model}. USB ID {pid}.",
                    model = d.display_name,
                    pid = d.lock.as_ref().map_or(String::new(), |l| usb_id(&l.locked.usb))
                ),
            ),
            (LockStatus::Unsupported(usb, None), _) => (
                tr!("This controller is not supported yet"),
                tr!("USB ID {pid}. Replug it to get XInput back.", pid = usb_id(usb)),
            ),
            _ => {
                (tr!("{name} is locked", name = name), tr!("While unlocked, games cannot see it."))
            }
        };
        LockedInfo {
            view,
            title: title.into(),
            body: body.into(),
            bar_title: if view == 3 {
                tr!("The controller did not unlock.").into()
            } else {
                "".into()
            },
            bar_body: bar_body.into(),
            guide: view == 4,
        }
    }
}

/// Pushes the locked panel and the Relock entry of the ☰ menu.
pub fn render_locked(state: &AppState, ui: &AppWindow) {
    ui.set_locked(state.locked_info());
    ui.global::<Relock<'_>>().set_available(state.relock_available());
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    /// The registered Ultimate.
    fn ultimate_model() -> &'static dyn controller_core::device::Model {
        controller_core::devices::model_named("Ultimate (BT)").unwrap()
    }
    use std::path::PathBuf;
    use std::sync::mpsc;

    use controller_core::devices::test_pad::UnvalidatedPad;
    use controller_core::devices::{pro3, ultimate};

    use super::*;
    use crate::state::tests::{connected, new_state, plug_ultimate, ultimate, PORT};

    fn target(port: &str) -> Target {
        Target { port: port.to_owned(), model: ultimate_model() }
    }

    /// A locked pad of the model that no one has checked on a real pad.
    fn unchecked(port: &str) -> Target {
        Target { port: port.to_owned(), model: &UnvalidatedPad }
    }

    fn failure(error: Error) -> Failure {
        Failure { error, holders: vec!["steam".to_owned()] }
    }

    fn first(s: &AppState) -> &LockStatus {
        &s.locked.first().unwrap().status
    }

    #[test]
    fn a_pad_keeps_its_status_while_it_stays_listed() {
        let (tx, rx) = mpsc::channel();
        let mut s = new_state();
        s.locked_seen(vec![target("1-2"), target("1-4")]);
        assert!(s.unlock_requested(&tx, "1-2"));
        assert_eq!(rx.try_recv().unwrap(), Command::Unlock("1-2".to_owned()));
        assert!(!s.unlock_requested(&tx, "1-2"), "one unlock at a time per pad");
        s.locked_seen(vec![target("1-2"), target("1-4")]);
        assert_eq!(first(&s), &LockStatus::Unlocking);
        assert_eq!(s.locked.get(1).unwrap().status, LockStatus::Locked);
    }

    #[test]
    fn a_failed_unlock_keeps_the_holders_and_can_be_tried_again() {
        let (tx, rx) = mpsc::channel();
        let mut s = new_state();
        s.locked_seen(vec![target("1-2")]);
        s.unlock_requested(&tx, "1-2");
        s.unlock_finished("1-2", Err(&failure(Error::Timeout)));
        assert_eq!(
            first(&s),
            &LockStatus::Failed {
                holders: vec!["steam".to_owned()],
                denied: false,
                timed_out: true
            }
        );
        assert!(s.unlock_requested(&tx, "1-2"));
        assert_eq!(rx.try_iter().count(), 2);
    }

    #[test]
    fn a_denied_unlock_says_so_and_check_again_unlocks_it_once_more() {
        let (tx, rx) = mpsc::channel();
        let mut s = new_state();
        s.locked_seen(vec![target("1-2")]);
        s.unlock_finished("1-2", Err(&failure(Error::PermissionDenied("x".to_owned()))));
        assert!(matches!(first(&s), LockStatus::Failed { denied: true, .. }));
        s.read_denied("1-2", crate::state::Rule::Missing);
        assert!(s.asks_for_rule());
        assert!(s.recheck_locks(&tx));
        assert!(!s.asks_for_rule(), "the screen clears until the answer comes");
        assert_eq!(first(&s), &LockStatus::Unlocking);
        assert_eq!(rx.try_recv().unwrap(), Command::Unlock("1-2".to_owned()));
        assert!(!s.recheck_locks(&tx), "nothing denied now");
    }

    #[test]
    fn an_unlock_that_works_leaves_the_list() {
        let mut s = new_state();
        s.locked_seen(vec![target("1-2")]);
        s.unlock_finished("1-2", Ok(&Unlocked::Port(PathBuf::from("/dev/hidraw1"))));
        assert!(s.locked.is_empty());
    }

    #[test]
    fn an_unsupported_pad_stays_until_it_is_unplugged() {
        let usb = UsbId { vendor: 0x2dc8, product: 0x1234 };
        let mut s = new_state();
        s.locked_seen(vec![target("1-2")]);
        s.unlock_finished("1-2", Ok(&Unlocked::Unsupported(usb, None)));
        s.locked_seen(vec![]);
        assert_eq!(first(&s), &LockStatus::Unsupported(usb, None));
        let (tx, _rx) = mpsc::channel();
        assert!(!s.unlock_requested(&tx, "1-2"), "nothing more is sent");
        s.unplugged("1-2");
        assert!(s.locked.is_empty(), "the message goes with the pad");
        assert!(!s.shows_pad());
    }

    #[test]
    fn an_unsupported_pad_that_comes_back_locked_starts_over() {
        let usb = UsbId { vendor: 0x2dc8, product: 0x1234 };
        let mut s = new_state();
        s.locked_seen(vec![target("1-2")]);
        s.unlock_finished("1-2", Ok(&Unlocked::Unsupported(usb, None)));
        s.locked_seen(vec![target("1-2")]);
        assert_eq!(first(&s), &LockStatus::Locked);
    }

    #[test]
    fn two_locked_pads_are_numbered_like_two_pro_3s() {
        let usb = UsbId { vendor: 0x2dc8, product: 0x1234 };
        let mut s = new_state();
        s.locked_seen(vec![target("1-2"), target("1-4")]);
        assert_eq!(
            s.picker_labels(),
            ["8BitDo Ultimate · Locked (1)", "8BitDo Ultimate · Locked (2)"]
        );
        s.unlock_finished("1-4", Ok(&Unlocked::Unsupported(usb, None)));
        assert_eq!(s.picker_labels(), ["8BitDo Ultimate · Locked", "8BitDo · Not supported"]);
        s.pick_entry(1);
        assert_eq!(s.lock_status().as_deref(), Some("Not supported yet"));
    }

    #[test]
    fn the_picker_lists_listed_controllers_before_locked_ones() {
        let mut s = ultimate("1.09");
        s.locked_seen(vec![target("1-2")]);
        assert_eq!(s.picker_labels(), ["Ultimate (BT) · XInput", "8BitDo Ultimate · Locked"]);
        assert!(s.shown_lock().is_none(), "the listed controller shows until a lock is picked");
        assert_eq!(s.picker_index(), 0);
        s.pick_entry(1);
        assert_eq!(s.picker_index(), 1);
        assert!(s.shown_lock().is_some() && s.active().is_none());
        s.pick_entry(0);
        assert!(s.shown_lock().is_none() && s.active().is_some());
    }

    #[test]
    fn the_panel_names_the_model_and_the_state_of_the_unlock() {
        let (tx, _rx) = mpsc::channel();
        let mut s = new_state();
        s.locked_seen(vec![target("1-2")]);
        let info = s.locked_info();
        assert_eq!(info.view, 1);
        assert!(!info.guide);
        assert_eq!(info.title, "8BitDo Ultimate is locked");
        assert_eq!(info.body, "While unlocked, games cannot see it.");
        assert_eq!(s.lock_status().as_deref(), Some("Locked"));
        s.unlock_requested(&tx, "1-2");
        assert_eq!(s.locked_info().view, 2);
        assert_eq!(s.lock_status().as_deref(), Some("Unlocking…"));
        s.unlock_finished("1-2", Err(&Failure { error: Error::Timeout, holders: vec![] }));
        let info = s.locked_info();
        assert_eq!(info.view, 3);
        assert_eq!(info.bar_title, "The controller did not unlock.");
        assert_eq!(info.bar_body, "Replug it if this repeats.");
        assert_eq!(s.lock_status().as_deref(), Some("Did not unlock"));
        s.unlock_finished("1-2", Err(&Failure { error: Error::Usb("x".into()), holders: vec![] }));
        assert_eq!(s.locked_info().bar_body, "", "no replug hint but for a timeout");
        s.unlock_finished(
            "1-2",
            Ok(&Unlocked::Unsupported(UsbId { vendor: 0x2dc8, product: 0x1234 }, None)),
        );
        let info = s.locked_info();
        assert_eq!((info.view, info.title.as_str()), (4, "This controller is not supported yet"));
        assert_eq!(info.body, "USB ID 2dc8:1234. Replug it to get XInput back.");
    }

    #[test]
    fn an_unvalidated_model_on_its_config_port_gets_the_refusal_screen() {
        let usb = UsbId { vendor: 0x2dc8, product: 0x7e58 };
        let mut s = new_state();
        s.refused_seen(unchecked("1-2"), usb);
        s.refused_seen(unchecked("1-2"), usb);
        assert_eq!(s.locked.len(), 1, "once per pad");
        assert!(s.shows_pad() && s.listed().next().is_none());
        let info = s.locked_info();
        assert_eq!(info.view, 4);
        assert_eq!(info.title, "Unchecked Pad is not tested yet");
        assert_eq!(info.body, "USB ID 2dc8:7e58.", "it never locked: no replug advice");
        assert!(info.guide);
        assert_eq!(s.picker_labels(), ["Unchecked Pad · Not tested"]);
        let (tx, rx) = mpsc::channel();
        assert!(!s.unlock_requested(&tx, "1-2"));
        assert!(rx.try_recv().is_err(), "nothing goes to the pad");
        s.unplugged("1-2");
        assert!(!s.shows_pad());
    }

    #[test]
    fn the_guide_link_points_at_the_repo_doc() {
        assert_eq!(
            GUIDE_URL,
            "https://github.com/LucasPicoli/8B/blob/master/docs/validating-a-controller.md"
        );
    }

    #[test]
    fn a_pad_of_an_unvalidated_model_is_refused_at_once_and_nothing_is_sent() {
        let (tx, rx) = mpsc::channel();
        let mut s = new_state();
        s.locked_seen(vec![unchecked("1-2")]);
        let info = s.locked_info();
        assert_eq!(info.view, 4);
        assert_eq!(info.title, "This Test family is not tested yet");
        assert_eq!(info.body, "It reports as Unchecked Pad. USB ID 2dc8:7e59.");
        assert!(info.guide, "the screen links to the guide");
        assert_eq!(s.lock_status().as_deref(), Some("Not tested yet"));
        assert_eq!(s.picker_labels(), ["Test family · Locked"]);
        assert!(!s.unlock_requested(&tx, "1-2"), "no unlock for a refused model");
        assert!(rx.try_recv().is_err());
        assert_eq!(first(&s), &LockStatus::Locked);
    }

    #[test]
    fn an_unlock_that_proves_an_unvalidated_model_names_it_and_one_that_proves_nothing_does_not() {
        let mut s = new_state();
        s.locked_seen(vec![target("1-2")]);
        let usb = UsbId { vendor: 0x2dc8, product: 0x7e58 };
        s.unlock_finished("1-2", Ok(&Unlocked::Unsupported(usb, Some("Unchecked Pad".to_owned()))));
        let info = s.locked_info();
        assert_eq!(info.view, 4);
        assert_eq!(info.title, "Unchecked Pad is not tested yet");
        assert_eq!(info.body, "USB ID 2dc8:7e58. Replug it to get XInput back.");
        assert!(info.guide);
        assert_eq!(s.picker_labels(), ["Unchecked Pad · Not tested"]);
        s.unlock_finished("1-2", Ok(&Unlocked::Unsupported(usb, None)));
        let info = s.locked_info();
        assert_eq!(info.title, "This controller is not supported yet");
        assert!(info.guide, "an unknown id gets the link too");
    }

    #[test]
    fn a_denied_unlock_shows_the_locked_panel_behind_the_access_screen() {
        let mut s = new_state();
        s.locked_seen(vec![target("1-2")]);
        s.unlock_finished("1-2", Err(&failure(Error::PermissionDenied("x".to_owned()))));
        assert_eq!(s.locked_info().view, 1);
        assert!(!s.shown_lock().unwrap().failed());
    }

    #[test]
    fn relock_shows_only_for_an_unlocked_pad_with_a_lock() {
        assert!(ultimate("1.09").relock_available());
        assert!(!crate::state::tests::connected(controller_core::devices::pro3::XINPUT)
            .relock_available());
        let mut s = ultimate("1.09");
        s.locked_seen(vec![target("1-2")]);
        s.pick_entry(1);
        assert!(!s.relock_available(), "a locked pad is shown");
    }

    #[test]
    fn relock_without_edits_goes_at_once_and_only_once() {
        let (tx, rx) = mpsc::channel();
        let mut s = ultimate("1.09");
        s.relock_clicked(&tx);
        assert!(!s.relock_asking);
        assert!(matches!(rx.try_recv().unwrap(), Command::Relock(t) if t.port == PORT));
        assert!(!s.relock_available(), "the entry goes while the relock runs");
        s.relock_clicked(&tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn relock_over_edits_asks_and_lists_this_pads_slots() {
        let (tx, rx) = mpsc::channel();
        let mut s = ultimate("1.09");
        s.set_output("p1", "auto turbo");
        s.relock_clicked(&tx);
        assert!(s.relock_asking && rx.try_recv().is_err());
        let info = s.relock_info();
        assert!(info.shown);
        assert_eq!((info.total, info.write), (1, 1));
        s.relock_cancelled();
        assert!(!s.relock_asking && !s.relock_info().shown);
        s.relock_clicked(&tx);
        s.relock_discarded(&tx);
        assert!(matches!(rx.try_recv().unwrap(), Command::Relock(_)));
    }

    #[test]
    fn the_question_closes_when_the_edits_go() {
        let (tx, _rx) = mpsc::channel();
        let mut s = ultimate("1.09");
        s.set_output("p1", "auto turbo");
        s.relock_clicked(&tx);
        s.discard();
        s.settle_relock();
        assert!(!s.relock_asking);
    }

    #[test]
    fn write_then_relock_waits_for_every_write() {
        let (tx, rx) = mpsc::channel();
        let mut s = ultimate("1.09");
        s.set_output("p1", "auto turbo");
        s.relock_clicked(&tx);
        assert_eq!(s.relock_write().as_deref(), Some(PORT));
        assert!(s.write.batch.is_some());
        assert_eq!(s.take_chained_relock(), None, "the batch is still open");
        // The batch ended with every write done: the relock follows.
        s.write.batch = None;
        assert_eq!(s.take_chained_relock().as_deref(), Some(PORT));
        assert_eq!(s.take_chained_relock(), None, "once");
        s.relock_target_sent(&tx, PORT);
        assert!(matches!(rx.try_recv().unwrap(), Command::Relock(_)));
    }

    #[test]
    fn a_failed_or_cancelled_write_drops_the_chained_relock() {
        let (tx, _rx) = mpsc::channel();
        let mut s = ultimate("1.09");
        s.set_output("p1", "auto turbo");
        s.relock_clicked(&tx);
        s.relock_write();
        s.batch_read(PORT, false);
        assert_eq!(s.relock_after_write, None, "the read before the review failed");
        s.relock_clicked(&tx);
        s.relock_write();
        s.batch_read(PORT, true);
        s.end_batch();
        assert_eq!(s.relock_after_write, None, "the review was cancelled");
    }

    #[test]
    fn a_relock_that_worked_lists_the_pad_locked_and_shows_it() {
        let (tx, _rx) = mpsc::channel();
        let mut s = ultimate("1.09");
        s.relock_clicked(&tx);
        s.relocked(PORT);
        assert_eq!(s.controllers, []);
        assert_eq!(s.locked.len(), 1);
        assert_eq!(s.locked_info().view, 1);
        assert_eq!(s.picker_labels(), ["8BitDo Ultimate · Locked"]);
        s.locked_seen(vec![target(PORT)]);
        assert_eq!(s.locked.len(), 1, "the worker's list agrees");
    }

    #[test]
    fn an_ultimate_unlocks_and_relocks_beside_a_pro_3_without_touching_it() {
        let (tx, rx) = mpsc::channel();
        let mut s = connected(pro3::XINPUT);
        s.set_name("Kept");
        s.select(2, 0);
        s.locked_seen(vec![target("1-2")]);
        assert_eq!(s.picker_labels(), ["Pro 3 · XInput", "8BitDo Ultimate · Locked"]);
        assert!(s.relock_targets().is_empty(), "the Pro 3 declares no lock");
        s.pick_entry(1);
        assert!(s.unlock_requested(&tx, "1-2"));
        s.unlock_finished("1-2", Ok(&Unlocked::Port(PathBuf::from("/dev/hidraw3"))));
        plug_ultimate(&mut s, "1-2", "1.09");
        assert_eq!(s.picker_labels(), ["Pro 3 · XInput", "Ultimate (BT) · XInput"]);
        assert_eq!(s.picker_index(), 1, "the unlocked pad shows once read");
        assert_eq!(s.selected_slot(), Some((ultimate::XINPUT, 1)), "in its current mode");
        let ports: Vec<_> = s.relock_targets().into_iter().map(|t| t.port).collect();
        assert_eq!(ports, ["1-2"], "a close relocks the Ultimate only");
        s.relock_clicked(&tx);
        s.relocked("1-2");
        assert_eq!(s.picker_labels(), ["Pro 3 · XInput", "8BitDo Ultimate · Locked"]);
        assert_eq!(s.picker_index(), 1, "the locked panel shows");
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(matches!(
            sent.as_slice(),
            [Command::Unlock(a), Command::Relock(b)] if a == "1-2" && b.port == "1-2"
        ));
        s.pick_entry(0);
        assert_eq!(
            s.slot(pro3::XINPUT, 1).shown().unwrap().name,
            "Kept",
            "the Pro 3 keeps its edit"
        );
    }

    #[test]
    fn an_unlock_that_ends_while_the_pro_3_shows_leaves_it_shown() {
        let (tx, _rx) = mpsc::channel();
        let mut s = connected(pro3::XINPUT);
        s.locked_seen(vec![target("1-2")]);
        s.pick_entry(1);
        s.unlock_requested(&tx, "1-2");
        s.pick_entry(0);
        s.unlock_finished("1-2", Ok(&Unlocked::Port(PathBuf::from("/dev/hidraw3"))));
        plug_ultimate(&mut s, "1-2", "1.09");
        assert_eq!(s.picker_labels(), ["Pro 3 · XInput", "Ultimate (BT) · XInput"]);
        assert_eq!(s.picker_index(), 0);
    }

    #[test]
    fn a_relock_that_ends_while_the_pro_3_shows_leaves_it_shown() {
        let (tx, _rx) = mpsc::channel();
        let mut s = connected(pro3::XINPUT);
        plug_ultimate(&mut s, "1-2", "1.09");
        s.pick_entry(1);
        s.relock_clicked(&tx);
        s.pick_entry(0);
        s.relocked("1-2");
        assert_eq!(s.picker_labels(), ["Pro 3 · XInput", "8BitDo Ultimate · Locked"]);
        assert_eq!(s.picker_index(), 0);
    }

    #[test]
    fn a_relock_that_failed_says_so_and_keeps_the_pad_unlocked() {
        let (tx, _rx) = mpsc::channel();
        let mut s = ultimate("1.09");
        s.relock_clicked(&tx);
        s.relock_failed(&Failure { error: Error::Timeout, holders: vec![] });
        assert!(s.notice.as_ref().is_some_and(|n| n.error));
        assert!(s.relock_available(), "it can be tried again");
        assert_eq!(s.controllers.len(), 1);
    }
}
