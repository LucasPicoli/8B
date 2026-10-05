//! Installs the udev rule that lets the user at the seat open the controller, and
//! the unit that keeps a pad in `XInput` mode connected.
//!
//! `pkexec` runs a shell line as root with the file texts passed as arguments, so
//! root never reads the `AppImage`'s FUSE mount. When it fails, the window shows
//! [`manual_command`](controller_core::transport::udev::manual_command) to run
//! instead.

use std::fs;
use std::process::Command;

use controller_core::transport::udev::{
    KEEPALIVE_UNIT, KEEPALIVE_UNIT_PATH, UDEV_RULE, UDEV_RULE_PATH,
};

use crate::state::Rule;

/// Writes `$1` to `$2` and `$3` to `$4`, reloads systemd and the rules, re-applies
/// them to present hidraw nodes, and waits until udev has set the new access, so
/// the next open needs no replug.
const INSTALL_SCRIPT: &str = "printf '%s' \"$1\" > \"$2\" \
    && printf '%s' \"$3\" > \"$4\" \
    && systemctl daemon-reload \
    && udevadm control --reload-rules \
    && udevadm trigger --subsystem-match=hidraw --action=change \
    && udevadm settle";

/// `pkexec` exit code when the password prompt was closed.
const PKEXEC_DISMISSED: i32 = 126;

/// `pkexec` exit code when no authorisation was given: a wrong password, no
/// authentication agent, or another polkit failure. KDE's agent also returns it
/// when its prompt is closed.
const PKEXEC_NOT_AUTHORISED: i32 = 127;

/// The installed files against the ones this build installs. A file that cannot
/// be read counts as missing.
#[must_use]
pub fn rule_state() -> Rule {
    let rule = fs::read_to_string(UDEV_RULE_PATH).ok();
    let unit = fs::read_to_string(KEEPALIVE_UNIT_PATH).ok();
    match (rule.as_deref(), unit.as_deref()) {
        (None, _) => Rule::Missing,
        (Some(UDEV_RULE), Some(KEEPALIVE_UNIT)) => Rule::Current,
        _ => Rule::Outdated,
    }
}

/// Installs the rule through `pkexec`. Blocks while the password prompt is open.
///
/// # Errors
/// Returns why the install failed, as a sentence for the window.
pub fn install_rule() -> Result<(), String> {
    let out = Command::new("pkexec")
        .args(["/bin/sh", "-c", INSTALL_SCRIPT, "sh", UDEV_RULE, UDEV_RULE_PATH])
        .args([KEEPALIVE_UNIT, KEEPALIVE_UNIT_PATH])
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "pkexec is not installed.".to_owned(),
            _ => format!("pkexec did not start: {e}."),
        })?;
    match out.status.code() {
        Some(0) => Ok(()),
        Some(PKEXEC_DISMISSED | PKEXEC_NOT_AUTHORISED) => {
            Err("The password prompt was closed, or the password was not accepted.".to_owned())
        }
        _ => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let last = stderr.lines().last().unwrap_or_default().trim();
            Err(format!("The install command failed ({}): {last}", out.status))
        }
    }
}
