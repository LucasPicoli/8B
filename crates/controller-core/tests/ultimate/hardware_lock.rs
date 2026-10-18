//! Unlock and relock on an attached Ultimate, locked, 10 times in a row. Sends the
//! declared sequences and opens one read session per unlock; writes no profile.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::time::{Duration, Instant};

use controller_core::detect::{scan_sysfs_all, usb_id, xpad_event};
use controller_core::device::ControllerSpec;
use controller_core::devices::ultimate::Ultimate;
use controller_core::devices::{locked_ports, SYSFS_USB};
use controller_core::transport::lock::{relock, unlock, Unlocked};
use controller_core::transport::{DeviceIo, HidrawDevice};

/// How many unlock and relock rounds to run.
const ROUNDS: usize = 10;
/// The pause between a relock and the next unlock.
const GAP: Duration = Duration::from_millis(1000);

/// Waits up to 5 s for `ready`.
fn eventually(ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
#[ignore = "needs a locked Ultimate on USB"]
fn unlock_and_relock_ten_times() {
    let lock = Ultimate.description().unwrap().lock.as_ref().unwrap();
    // A pad left unlocked by an earlier run starts locked again.
    let ports = &Ultimate.description().unwrap().config_ports;
    for left in scan_sysfs_all(Path::new(SYSFS_USB), ports) {
        relock(&Ultimate, left.port_path()).unwrap();
        assert!(eventually(|| locked_ports().iter().any(|l| l.port_path == left.port_path())));
    }
    for round in 1..=ROUNDS {
        let locked = locked_ports().into_iter().next().unwrap_or_else(|| {
            panic!("round {round}: no locked Ultimate is attached");
        });
        let port = locked.port_path.clone();
        let started = Instant::now();
        let node = match unlock(&locked) {
            Ok(Unlocked::Port(node)) => node,
            other => panic!("round {round}: {other:?}"),
        };
        println!("round {round}: unlocked {port} at {} in {:?}", node.display(), started.elapsed());
        let model = HidrawDevice::at(&port).model().unwrap();
        assert_eq!(model.description().unwrap().short_name, "Ultimate", "round {round}");

        relock(&Ultimate, &port).unwrap();
        let dir = Path::new(SYSFS_USB).join(&port);
        assert_eq!(usb_id(&dir), Some(lock.locked.usb), "round {round}");
        assert!(eventually(|| xpad_event(&dir).is_some()), "round {round}: xpad is back");
        // udev applies the seat access a moment after the node appears.
        assert!(eventually(|| locked_ports().iter().any(|l| l.port_path == port)), "round {round}");
        std::thread::sleep(GAP);
    }
}
