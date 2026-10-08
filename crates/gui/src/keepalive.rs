//! Holds the `xpad` event node of a controller open while 8B runs. A Pro 3 in `XInput`
//! mode resets about half a second after nothing polls it, and `xpad` polls a wired pad
//! only while its event node is open. With the node held, the pad stays put while the
//! user edits it, with no udev keepalive installed.
//!
//! Inside the Flatpak sandbox 8B cannot see whether the udev fix is installed, so it
//! watches a new pad for a few seconds before it holds the node. A pad that reconnects
//! in that time is one that needs the fix, and the verdict says so.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::PathBuf;

use controller_core::detect::{xpad_event, DetectedUsb};
use log::warn;

/// Polls a new pad is watched before its node is held, about 4 s, when the sandbox hides
/// the fix files. The pad that needs the fix resets 0.5 s after it connects and returns
/// 1.6 s later, so the window sees it twice.
const WATCH_POLLS: u8 = 8;

/// Device numbers in a row, after the first, that mark a pad as reconnecting. One is a
/// replug by hand.
const LOOP_CHANGES: u8 = 2;

/// One controller whose event node should be held: its port path, its USB device
/// number and the node to open.
type Wanted = (String, Option<u16>, PathBuf);

/// The controllers in `found` whose port sets `needs_keepalive` and whose event node
/// exists yet. A replug gets a new device number, so it is a new entry.
fn wanted(found: &[DetectedUsb], devnum: impl Fn(&DetectedUsb) -> Option<u16>) -> Vec<Wanted> {
    found
        .iter()
        .filter(|usb| usb.port.needs_keepalive)
        .filter_map(|usb| {
            let node = xpad_event(std::path::Path::new(&usb.sysfs_path))?;
            Some((usb.port_path().to_owned(), devnum(usb), node))
        })
        .collect()
}

/// What the watch of a new pad found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The USB port path of the pad.
    pub port: String,
    /// The pad reconnected while it was watched. Always `false` outside the sandbox,
    /// which does not watch.
    pub looping: bool,
}

/// A pad being watched: the device number last seen and how often it changed.
#[derive(Debug)]
struct Watch {
    num: Option<u16>,
    polls: u8,
    changes: u8,
}

/// The event nodes held open, by port path. A failed open is kept as `None`, so it is
/// logged once and tried again on each poll until the controller is replugged.
#[derive(Debug, Default)]
pub struct Keepalive {
    /// Polls to watch a new pad before holding it. Zero holds at once.
    watch_polls: u8,
    watching: BTreeMap<String, Watch>,
    /// The ports whose verdict was given and that are still present.
    judged: BTreeSet<String>,
    /// The ports whose failed hold was logged, until the port is no longer judged.
    warned: BTreeSet<String>,
    held: BTreeMap<String, (Option<u16>, Option<File>)>,
}

impl Keepalive {
    /// A holder that watches each new pad first when `sandboxed`, because the sandbox
    /// cannot read the fix files.
    #[must_use]
    pub fn new(sandboxed: bool) -> Self {
        Self { watch_polls: if sandboxed { WATCH_POLLS } else { 0 }, ..Self::default() }
    }

    /// Opens the event node of each new controller in `found` and drops the handle of
    /// each one that is gone or replugged. Call it on every poll. Returns the verdict
    /// of each pad whose watch ended, once per connection.
    pub fn sync(
        &mut self,
        found: &[DetectedUsb],
        devnum: impl Fn(&DetectedUsb) -> Option<u16>,
    ) -> Vec<Verdict> {
        let present: Vec<(String, Option<u16>)> = found
            .iter()
            .filter(|usb| usb.port.needs_keepalive)
            .map(|usb| (usb.port_path().to_owned(), devnum(usb)))
            .collect();
        let verdicts = self.watch(&present);
        self.hold(found, devnum);
        verdicts
    }

    /// Whether a pad was judged and its event node is not held yet. The worker then
    /// calls [`Self::hold`] between polls: a pad that reconnects every 1.6 s keeps its
    /// node for about half a second, so a hold tried once per poll mostly misses it.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.judged.iter().any(|port| !matches!(self.held.get(port), Some((_, Some(_)))))
    }

    /// Opens the event node of each judged pad that is not held yet, and drops the
    /// handle of each one that is gone or replugged.
    pub fn hold(&mut self, found: &[DetectedUsb], devnum: impl Fn(&DetectedUsb) -> Option<u16>) {
        let wanted = wanted(found, devnum);
        self.held.retain(|port, (num, _)| wanted.iter().any(|(p, n, _)| p == port && n == num));
        for (port, num, node) in wanted {
            if !self.judged.contains(&port) || matches!(self.held.get(&port), Some((_, Some(_)))) {
                continue;
            }
            // A node just made may not have its access entry yet, so a failed open is
            // tried again. It is logged once per judged connection.
            let file = File::open(&node)
                .map_err(|e| {
                    if self.warned.insert(port.clone()) {
                        warn!("cannot hold {}: {e}", node.display());
                    }
                })
                .ok();
            self.held.insert(port, (num, file));
        }
    }

    /// Advances the watch of each pad in `present` (port path and device number).
    fn watch(&mut self, present: &[(String, Option<u16>)]) -> Vec<Verdict> {
        self.judged.retain(|port| present.iter().any(|(p, _)| p == port));
        self.warned.retain(|port| self.judged.contains(port));
        let mut verdicts = Vec::new();
        for (port, num) in present {
            if self.judged.contains(port) {
                continue;
            }
            if self.watch_polls == 0 {
                self.judged.insert(port.clone());
                verdicts.push(Verdict { port: port.clone(), looping: false });
            } else {
                self.watching.entry(port.clone()).or_insert(Watch {
                    num: *num,
                    polls: 0,
                    changes: 0,
                });
            }
        }
        let watch_polls = self.watch_polls;
        let judged = &mut self.judged;
        self.watching.retain(|port, w| {
            let seen = present.iter().find(|(p, _)| p == port);
            if let Some((_, num)) = seen {
                if *num != w.num {
                    w.changes = w.changes.saturating_add(1);
                    w.num = *num;
                }
            }
            w.polls = w.polls.saturating_add(1);
            if w.polls < watch_polls {
                return true;
            }
            let looping = w.changes >= LOOP_CHANGES;
            // A pad caught in a gap of its loop is judged when it returns; one that
            // went away without looping starts a new watch when it comes back.
            if seen.is_none() {
                return looping;
            }
            judged.insert(port.clone());
            verdicts.push(Verdict { port: port.clone(), looping });
            false
        });
        verdicts
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use std::cell::Cell;

    use controller_core::detect::scan_sysfs_all;
    use controller_core::device::ControllerSpec as _;
    use controller_core::devices::pro3::Pro3;

    use super::*;

    /// Plugs a device with USB id `vendor:product` at `port`, with an `xpad` event node.
    fn plug(root: &std::path::Path, port: &str, vendor: &str, product: &str) {
        let dev = root.join(port);
        fs::create_dir_all(dev.join(format!("{port}:1.0/input/input9/event9"))).unwrap();
        fs::write(dev.join("idVendor"), vendor).unwrap();
        fs::write(dev.join("idProduct"), product).unwrap();
        symlink("../../bus/usb/drivers/xpad", dev.join(format!("{port}:1.0/driver"))).unwrap();
    }

    #[test]
    fn only_ports_with_the_flag_are_held() {
        let root = tempfile::tempdir().unwrap();
        plug(root.path(), "3-1", "2dc8", "310b"); // XInput: needs the keepalive
        plug(root.path(), "3-2", "2dc8", "6009"); // DInput: does not
        let ports = &Pro3.description().unwrap().config_ports;
        let found = scan_sysfs_all(root.path(), ports);
        let held = wanted(&found, |_| Some(7));
        assert_eq!(held, [("3-1".to_owned(), Some(7), PathBuf::from("/dev/input/event9"))]);
    }

    /// One pad on port `3-1` in `XInput`, as the scan finds it.
    fn xinput_pad(root: &std::path::Path) -> Vec<DetectedUsb> {
        plug(root, "3-1", "2dc8", "310b");
        scan_sysfs_all(root, &Pro3.description().unwrap().config_ports)
    }

    #[test]
    fn outside_the_sandbox_a_pad_is_judged_and_held_at_once() {
        let root = tempfile::tempdir().unwrap();
        let found = xinput_pad(root.path());
        let mut k = Keepalive::new(false);
        assert_eq!(
            k.sync(&found, |_| Some(7)),
            [Verdict { port: "3-1".to_owned(), looping: false }]
        );
        assert!(k.held.contains_key("3-1"));
        assert_eq!(k.sync(&found, |_| Some(7)), [], "one verdict per connection");
    }

    #[test]
    fn a_steady_pad_is_held_only_after_its_watch() {
        let root = tempfile::tempdir().unwrap();
        let found = xinput_pad(root.path());
        let mut k = Keepalive::new(true);
        for _ in 1..WATCH_POLLS {
            assert_eq!(k.sync(&found, |_| Some(7)), []);
            assert!(k.held.is_empty());
        }
        assert_eq!(
            k.sync(&found, |_| Some(7)),
            [Verdict { port: "3-1".to_owned(), looping: false }]
        );
        assert!(k.held.contains_key("3-1"));
    }

    #[test]
    fn a_pad_that_keeps_reconnecting_is_flagged() {
        let root = tempfile::tempdir().unwrap();
        let found = xinput_pad(root.path());
        let mut k = Keepalive::new(true);
        // A new device number every 3 polls (1.5 s), as the loop gives.
        let polls = Cell::new(0_u16);
        let mut last = Vec::new();
        for _ in 0..WATCH_POLLS {
            polls.set(polls.get() + 1);
            last = k.sync(&found, |_| Some(10 + polls.get() / 3));
        }
        assert_eq!(last, [Verdict { port: "3-1".to_owned(), looping: true }]);
    }

    #[test]
    fn one_replug_by_hand_is_not_a_loop() {
        let root = tempfile::tempdir().unwrap();
        let found = xinput_pad(root.path());
        let mut k = Keepalive::new(true);
        let polls = Cell::new(0_u16);
        let mut last = Vec::new();
        for _ in 0..WATCH_POLLS {
            polls.set(polls.get() + 1);
            last = k.sync(&found, |_| Some(if polls.get() < 4 { 7 } else { 8 }));
        }
        assert_eq!(last, [Verdict { port: "3-1".to_owned(), looping: false }]);
    }

    #[test]
    fn a_pad_that_leaves_and_returns_gets_a_new_verdict() {
        let root = tempfile::tempdir().unwrap();
        let found = xinput_pad(root.path());
        let mut k = Keepalive::new(false);
        assert_eq!(k.sync(&found, |_| Some(7)).len(), 1);
        assert_eq!(k.sync(&[], |_| None), []);
        assert_eq!(k.sync(&found, |_| Some(8)).len(), 1);
    }

    #[test]
    fn a_node_that_is_not_there_yet_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("3-1")).unwrap();
        fs::write(root.path().join("3-1/idVendor"), "2dc8").unwrap();
        fs::write(root.path().join("3-1/idProduct"), "310b").unwrap();
        let ports = &Pro3.description().unwrap().config_ports;
        let found = scan_sysfs_all(root.path(), ports);
        assert_eq!(wanted(&found, |_| None).len(), 0);
    }
}
