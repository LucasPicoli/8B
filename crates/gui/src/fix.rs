//! The offer of the keepalive fix: which attached controllers get it, and the dialog
//! that explains it. The text of the offer and the install live in the window and
//! [`crate::udev`].

use crate::state::{AppState, Install, Notice};

impl AppState {
    /// The worker watched the controller on `port`. `offer` says it needs the fix and
    /// the fix is not in place. A new connection is judged again, so a controller
    /// put off earlier is offered again.
    pub fn fix_verdict(&mut self, port: &str, offer: bool) {
        if offer {
            self.fix_offered.insert(port.to_owned());
        } else {
            self.fix_gone(port);
        }
    }

    /// The controller on `port` went away, or no longer needs the offer.
    pub fn fix_gone(&mut self, port: &str) {
        self.fix_offered.remove(port);
        if !self.fix_shown() {
            self.fix_open = false;
        }
    }

    /// Whether the offer shows: the picked controller is one it was made for.
    #[must_use]
    pub fn fix_shown(&self) -> bool {
        self.active().is_some_and(|c| self.fix_offered.contains(&c.port))
    }

    /// "Not now": the picked controller is not offered the fix again until it is
    /// plugged in anew.
    pub fn fix_put_off(&mut self) {
        if let Some(port) = self.active().map(|c| c.port.clone()) {
            self.fix_gone(&port);
        }
        self.fix_install = Install::Idle;
    }

    /// The dialog was closed. An install that failed is forgotten, so its error does not
    /// greet the next opening. `in_place` says the files are there now, as when the user
    /// ran the command in a terminal: no offer is left.
    pub fn fix_closed(&mut self, in_place: bool) {
        self.fix_open = false;
        if matches!(self.fix_install, Install::Failed(_)) {
            self.fix_install = Install::Idle;
        }
        if in_place {
            self.fix_offered.clear();
        }
    }

    /// The fix install was sent to the system.
    pub fn fix_install_started(&mut self) {
        self.fix_install = Install::Running;
    }

    /// The fix install came back. It covers every controller, so no offer is left.
    pub fn fix_install_finished(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.fix_offered.clear();
                self.fix_open = false;
                self.fix_install = Install::Idle;
                self.notice = Some(Notice {
                    error: false,
                    title: "The files are added.".to_owned(),
                    body: "The controller stays connected now.".to_owned(),
                });
            }
            Err(e) => self.fix_install = Install::Failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use controller_core::model::Mode;

    use super::*;
    use crate::state::tests::{full_read, new_state, PORT};

    fn connected() -> AppState {
        let mut s = new_state();
        s.presence(PORT, Some(Mode::XInput));
        s.read_finished(PORT, Ok(full_read()));
        s
    }

    #[test]
    fn the_offer_shows_for_the_picked_controller_until_put_off() {
        let mut s = connected();
        assert!(!s.fix_shown());
        s.fix_verdict(PORT, true);
        assert!(s.fix_shown());
        s.fix_put_off();
        assert!(!s.fix_shown());
        s.fix_verdict(PORT, true);
        assert!(s.fix_shown(), "a new connection is offered again");
    }

    #[test]
    fn closing_forgets_a_failed_install_and_ends_the_offer_when_the_files_are_there() {
        let mut s = connected();
        s.fix_verdict(PORT, true);
        s.fix_open = true;
        s.fix_install_finished(Err("closed".to_owned()));
        s.fix_closed(false);
        assert!(!s.fix_open && s.fix_shown());
        assert_eq!(s.fix_install, Install::Idle);
        s.fix_closed(true);
        assert!(!s.fix_shown());
    }

    #[test]
    fn unplugging_ends_the_offer_and_closes_the_dialog() {
        let mut s = connected();
        s.fix_verdict(PORT, true);
        s.fix_open = true;
        s.presence(PORT, None);
        assert!(s.fix_offered.is_empty() && !s.fix_open);
    }

    #[test]
    fn a_failed_install_keeps_the_offer_and_a_good_one_ends_it() {
        let mut s = connected();
        s.fix_verdict(PORT, true);
        s.fix_open = true;
        s.fix_install_started();
        s.fix_install_finished(Err("The password prompt was closed.".to_owned()));
        assert!(s.fix_shown() && s.fix_open);
        assert!(matches!(s.fix_install, Install::Failed(_)));
        s.fix_install_started();
        s.fix_install_finished(Ok(()));
        assert!(!s.fix_shown() && !s.fix_open);
        assert_eq!(s.fix_install, Install::Idle);
        assert!(s.notice.is_some());
    }
}
