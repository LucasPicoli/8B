//! Installs the udev rule that lets the user at the seat open the controller.
//!
//! `pkexec` runs a shell line as root with the rule text passed as an argument, so
//! root never reads the `AppImage`'s FUSE mount. When it fails, the window shows
//! [`UDEV_MANUAL_COMMAND`](controller_core::transport::udev::UDEV_MANUAL_COMMAND)
//! to run instead.

use std::path::Path;
use std::process::Command;

use controller_core::transport::udev::{UDEV_RULE, UDEV_RULE_PATH};

/// Writes `$1` to `$2`, reloads the rules, re-applies them to present hidraw nodes,
/// and waits until udev has set the new access, so the next open needs no replug.
const INSTALL_SCRIPT: &str = "printf '%s' \"$1\" > \"$2\" \
    && udevadm control --reload-rules \
    && udevadm trigger --subsystem-match=hidraw --action=change \
    && udevadm settle";

/// `pkexec` exit code when the password prompt was closed.
const PKEXEC_DISMISSED: i32 = 126;

/// `pkexec` exit code when no authorisation was given: a wrong password, no
/// authentication agent, or another polkit failure. KDE's agent also returns it
/// when its prompt is closed.
const PKEXEC_NOT_AUTHORISED: i32 = 127;

/// Whether the rule file is in place.
#[must_use]
pub fn rule_installed() -> bool {
    Path::new(UDEV_RULE_PATH).exists()
}

/// Installs the rule through `pkexec`. Blocks while the password prompt is open.
///
/// # Errors
/// Returns why the install failed, as a sentence for the window.
pub fn install_rule() -> Result<(), String> {
    let out = Command::new("pkexec")
        .args(["/bin/sh", "-c", INSTALL_SCRIPT, "sh", UDEV_RULE, UDEV_RULE_PATH])
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
