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
    /// The unlock brought up a USB id no description lists. Nothing more is sent;
    /// replugging returns the pad to `XInput`.
    Unsupported(UsbId),
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
                let status = old
                    .iter()
                    .find(|p| p.port == t.port)
                    .map(|p| p.status.clone())
                    .filter(|s| !matches!(s, LockStatus::Unsupported(_)));
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
                matches!(p.status, LockStatus::Unsupported(_))
                    && !self.locked.iter().any(|q| q.port == p.port)
            })
            .collect();
        self.locked.extend(kept);
    }

    /// An unsupported pad on `port` left the bus: its message goes.
    pub fn unplugged(&mut self, port: &str) {
        self.locked.retain(|p| p.port != port);
    }

    /// The user asked to unlock the pad on `port`. A pad already unlocking, or
    /// unsupported, is left alone. Returns whether the command went out.
    pub fn unlock_requested(&mut self, commands: &Sender<Command>, port: &str) -> bool {
        let Some(pad) = self.locked.iter().find(|p| p.port == port) else { return false };
        if matches!(pad.status, LockStatus::Unlocking | LockStatus::Unsupported(_)) {
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
                self.locked.retain(|p| p.port != port);
                // The controller shows once its read is back.
                if self.locked_shown.as_deref() == Some(port) {
                    self.locked_shown = None;
                }
                self.active_port = Some(port.to_owned());
            }
            Ok(Unlocked::Unsupported(usb)) => {
                self.set_lock_status(port, LockStatus::Unsupported(*usb));
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
                if matches!(p.status, LockStatus::Unsupported(_)) {
                    return tr!("8BitDo · Not supported");
                }
                let name = p.model.description().map_or("", |d| d.short_name.as_str());
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
    /// locked panel shows once the worker lists it.
    pub fn relocked(&mut self, port: &str) {
        self.relock_asking = false;
        self.relock_sent = None;
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
        self.locked_shown = Some(port.to_owned());
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
        Some(match self.shown_lock()?.status {
            LockStatus::Locked => tr!("Locked"),
            LockStatus::Unlocking => tr!("Unlocking…"),
            LockStatus::Failed { denied: true, .. } => tr!("Locked"),
            LockStatus::Failed { .. } => tr!("Did not unlock"),
            LockStatus::Unsupported(_) => tr!("Not supported yet"),
        })
    }

    /// What the locked panel shows, `view` 0 when it does not.
    #[must_use]
    pub fn locked_info(&self) -> LockedInfo {
        let Some(pad) = self.shown_lock() else { return LockedInfo::default() };
        let name = pad.model.description().map_or("", |d| d.display_name.as_str());
        let (view, bar_body) = match &pad.status {
            LockStatus::Locked | LockStatus::Failed { denied: true, .. } => (1, String::new()),
            LockStatus::Unlocking => (2, String::new()),
            LockStatus::Failed { holders, timed_out, .. } => {
                let mut body = holders::hint(holders, self.sandboxed).unwrap_or_default();
                if *timed_out {
                    if !body.is_empty() {
                        body.push(' ');
                    }
                    body.push_str(&tr!("Replug it if this keeps happening."));
                }
                (3, body)
            }
            LockStatus::Unsupported(_) => (4, String::new()),
        };
        let (title, body) = match &pad.status {
            LockStatus::Unsupported(usb) => (
                tr!("This controller is not supported yet"),
                tr!(
                    "USB ID {pid}. Replug it to get XInput back.",
                    pid = format!("{:04x}:{:04x}", usb.vendor, usb.product)
                ),
            ),
            _ => (
                tr!("{name} is locked", name = name),
                tr!("Unlocking takes it away from games until you relock it."),
            ),
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
    use std::path::PathBuf;
    use std::sync::mpsc;

    use controller_core::devices::ultimate::Ultimate;

    use super::*;
    use crate::state::tests::{new_state, ultimate, PORT};

    fn target(port: &str) -> Target {
        Target { port: port.to_owned(), model: &Ultimate }
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
        s.unlock_finished("1-2", Ok(&Unlocked::Unsupported(usb)));
        s.locked_seen(vec![]);
        assert_eq!(first(&s), &LockStatus::Unsupported(usb));
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
        s.unlock_finished("1-2", Ok(&Unlocked::Unsupported(usb)));
        s.locked_seen(vec![target("1-2")]);
        assert_eq!(first(&s), &LockStatus::Locked);
    }

    #[test]
    fn two_locked_pads_are_numbered_like_two_pro_3s() {
        let usb = UsbId { vendor: 0x2dc8, product: 0x1234 };
        let mut s = new_state();
        s.locked_seen(vec![target("1-2"), target("1-4")]);
        assert_eq!(s.picker_labels(), ["Ultimate · Locked (1)", "Ultimate · Locked (2)"]);
        s.unlock_finished("1-4", Ok(&Unlocked::Unsupported(usb)));
        assert_eq!(s.picker_labels(), ["Ultimate · Locked", "8BitDo · Not supported"]);
        s.pick_entry(1);
        assert_eq!(s.lock_status().as_deref(), Some("Not supported yet"));
    }

    #[test]
    fn the_picker_lists_listed_controllers_before_locked_ones() {
        let mut s = ultimate("1.09");
        s.locked_seen(vec![target("1-2")]);
        assert_eq!(s.picker_labels(), ["Ultimate · XInput", "Ultimate · Locked"]);
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
        assert_eq!(info.title, "Ultimate Bluetooth is locked");
        assert_eq!(info.body, "Unlocking takes it away from games until you relock it.");
        assert_eq!(s.lock_status().as_deref(), Some("Locked"));
        s.unlock_requested(&tx, "1-2");
        assert_eq!(s.locked_info().view, 2);
        assert_eq!(s.lock_status().as_deref(), Some("Unlocking…"));
        s.unlock_finished("1-2", Err(&Failure { error: Error::Timeout, holders: vec![] }));
        let info = s.locked_info();
        assert_eq!(info.view, 3);
        assert_eq!(info.bar_title, "The controller did not unlock.");
        assert_eq!(info.bar_body, "Replug it if this keeps happening.");
        assert_eq!(s.lock_status().as_deref(), Some("Did not unlock"));
        s.unlock_finished("1-2", Err(&Failure { error: Error::Usb("x".into()), holders: vec![] }));
        assert_eq!(s.locked_info().bar_body, "", "no replug hint but for a timeout");
        s.unlock_finished(
            "1-2",
            Ok(&Unlocked::Unsupported(UsbId { vendor: 0x2dc8, product: 0x1234 })),
        );
        let info = s.locked_info();
        assert_eq!((info.view, info.title.as_str()), (4, "This controller is not supported yet"));
        assert_eq!(info.body, "USB ID 2dc8:1234. Replug it to get XInput back.");
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
        assert_eq!(s.picker_labels(), ["Ultimate · Locked"]);
        s.locked_seen(vec![target(PORT)]);
        assert_eq!(s.locked.len(), 1, "the worker's list agrees");
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
