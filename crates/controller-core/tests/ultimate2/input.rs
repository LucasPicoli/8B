//! What the pad sends while a person presses it, read from its evdev node.
//!
//! A reader thread turns each event into a named control: a key such as `BTN_A`, a
//! trigger pulled past half (`LT`, `RT`), or a stick pushed past half (`left stick
//! right`). [`Input::gesture`] waits, with no timeout, for the first one, then gathers
//! the rest until every control is back at rest for [`SETTLE`].
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::Duration;

/// How long every control must rest before a gesture counts as over. A macro step of
/// the check lasts 100 ms.
const SETTLE: Duration = Duration::from_millis(700);
/// Bytes of one `struct input_event` on a 64-bit kernel: a 16-byte time, then type,
/// code and value.
const EVENT_LEN: usize = 24;
/// `EV_KEY` and `EV_ABS`, the event types the check reads.
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
/// A stick past this, of its ±32767 range, counts as pushed.
const STICK_PUSHED: i32 = 16_000;
/// A trigger past this, of its 0 to 255 range, counts as pulled.
const TRIGGER_PULLED: i32 = 128;

/// The key names the check expects, by code (`linux/input-event-codes.h`).
const KEYS: [(u16, &str); 13] = [
    (0x130, "BTN_A"),
    (0x131, "BTN_B"),
    (0x133, "BTN_X"),
    (0x134, "BTN_Y"),
    (0x136, "BTN_TL"),
    (0x137, "BTN_TR"),
    (0x138, "BTN_TL2"),
    (0x139, "BTN_TR2"),
    (0x13a, "BTN_SELECT"),
    (0x13b, "BTN_START"),
    (0x13c, "BTN_MODE"),
    (0x13d, "BTN_THUMBL"),
    (0x13e, "BTN_THUMBR"),
];

/// One event: type, code, value.
type Event = (u16, u16, i32);

/// The pad's input, read on a thread of its own.
pub struct Input {
    events: Receiver<Event>,
    /// The controls held now, by name.
    held: BTreeSet<String>,
}

/// The evdev node of the pad on USB port path `port`, such as `8-5`: the event node
/// of its interface 0, where the kernel's gamepad driver binds.
fn node(port: &str) -> PathBuf {
    let dir = PathBuf::from(format!("/sys/bus/usb/devices/{port}:1.0/input"));
    let inputs = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for input in inputs.flatten() {
        for entry in std::fs::read_dir(input.path()).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("event") {
                return PathBuf::from("/dev/input").join(name);
            }
        }
    }
    panic!("no event node under {}", dir.display());
}

impl Input {
    /// Starts reading the pad on `port`.
    pub fn open(port: &str) -> Self {
        let path = node(port);
        let mut file = std::fs::File::open(&path).unwrap_or_else(|e| {
            panic!("{}: {e}; run the check from a desktop session", path.display())
        });
        let (send, events) = channel();
        // The thread ends when the pad leaves USB or the check ends.
        std::thread::spawn(move || {
            let mut raw = [0u8; EVENT_LEN];
            while file.read_exact(&mut raw).is_ok() {
                let kind = u16::from_ne_bytes([raw[16], raw[17]]);
                let code = u16::from_ne_bytes([raw[18], raw[19]]);
                let value = i32::from_ne_bytes([raw[20], raw[21], raw[22], raw[23]]);
                if send.send((kind, code, value)).is_err() {
                    return;
                }
            }
        });
        Self { events, held: BTreeSet::new() }
    }

    /// Drops every event so far, so a gesture starts clean.
    pub fn forget(&mut self) {
        while self.events.try_recv().is_ok() {}
        self.held.clear();
    }

    /// The controls `event` presses, and updates what is held.
    fn take(&mut self, (kind, code, value): Event) -> Vec<String> {
        let changes: Vec<(String, bool)> = match kind {
            EV_KEY => vec![(key_name(code), value != 0)],
            EV_ABS => axis(code, value),
            _ => Vec::new(),
        };
        let mut pressed = Vec::new();
        for (name, down) in changes {
            if down && self.held.insert(name.clone()) {
                pressed.push(name);
            } else if !down {
                self.held.remove(&name);
            }
        }
        pressed
    }

    /// Every control pressed in the next gesture, in order. Waits as long as it takes
    /// for the first one.
    pub fn gesture(&mut self) -> Vec<String> {
        let mut pressed = Vec::new();
        while pressed.is_empty() {
            let event = self.events.recv().expect("the pad left USB");
            pressed = self.take(event);
        }
        loop {
            match self.events.recv_timeout(SETTLE) {
                Ok(event) => pressed.extend(self.take(event)),
                Err(RecvTimeoutError::Timeout) if self.held.is_empty() => return pressed,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => panic!("the pad left USB"),
            }
        }
    }

    /// Every control pressed since [`Self::forget`], without waiting.
    pub fn so_far(&mut self) -> Vec<String> {
        let mut pressed = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            pressed.extend(self.take(event));
        }
        pressed
    }
}

/// The name of key `code`.
fn key_name(code: u16) -> String {
    KEYS.iter()
        .find(|(c, _)| *c == code)
        .map_or_else(|| format!("key 0x{code:x}"), |(_, n)| (*n).to_owned())
}

/// The controls axis `code` at `value` holds or lets go. A stick names each direction,
/// so a push and its release are one control each way.
fn axis(code: u16, value: i32) -> Vec<(String, bool)> {
    let stick = |name: &str, low: &str, high: &str| {
        vec![
            (format!("{name} {low}"), value < -STICK_PUSHED),
            (format!("{name} {high}"), value > STICK_PUSHED),
        ]
    };
    match code {
        0 => stick("left stick", "left", "right"),
        1 => stick("left stick", "up", "down"),
        3 => stick("right stick", "left", "right"),
        4 => stick("right stick", "up", "down"),
        2 => vec![("LT".to_owned(), value > TRIGGER_PULLED)],
        5 => vec![("RT".to_owned(), value > TRIGGER_PULLED)],
        _ => Vec::new(),
    }
}
