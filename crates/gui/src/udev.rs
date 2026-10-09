//! Installs the udev rules through `pkexec`: the access rule that lets the user at the
//! seat open the controller, and the keepalive fix that keeps a pad connected.
//!
//! `pkexec` runs a shell line as root with the file texts passed as arguments, so
//! root never reads the `AppImage`'s FUSE mount. When it fails, the window shows
//! [`manual_command`](controller_core::transport::udev::manual_command) to run
//! instead.

use std::fs;
use std::path::Path;
use std::process::Command;

use controller_core::transport::udev::{
    access_rule, keepalive_rule, rule_0_1_0_is_current, KEEPALIVE_RULE_PATH, KEEPALIVE_UNIT,
    KEEPALIVE_UNIT_PATH, UDEV_RULE_0_1_0, UDEV_RULE_PATH,
};

use crate::state::Rule;

/// Writes `$1` to `$2`, reloads the rules, re-applies them to present hidraw nodes,
/// and waits until udev has set the new access, so the next open needs no replug.
const ACCESS_SCRIPT: &str = "printf '%s' \"$1\" > \"$2\" \
    && udevadm control --reload-rules \
    && udevadm trigger --subsystem-match=hidraw --action=change \
    && udevadm settle";

/// Writes `$1` to `$2` and `$3` to `$4`, reloads systemd and the rules, and
/// re-applies them to present input nodes, so the unit starts with no replug.
const KEEPALIVE_SCRIPT: &str = "printf '%s' \"$1\" > \"$2\" \
    && printf '%s' \"$3\" > \"$4\" \
    && systemctl daemon-reload \
    && udevadm control --reload-rules \
    && udevadm trigger --subsystem-match=input --action=change \
    && udevadm settle";

/// `pkexec` exit code when the password prompt was closed.
const PKEXEC_DISMISSED: i32 = 126;

/// `pkexec` exit code when no authorisation was given: a wrong password, no
/// authentication agent, or another polkit failure. KDE's agent also returns it
/// when its prompt is closed.
const PKEXEC_NOT_AUTHORISED: i32 = 127;

/// Where Flatpak marks its sandbox.
const FLATPAK_INFO: &str = "/.flatpak-info";

/// Whether the app runs in the Flatpak sandbox.
#[must_use]
pub fn sandboxed() -> bool {
    Path::new(FLATPAK_INFO).exists()
}

/// The installed access rule against the one this build installs. A file that
/// cannot be read counts as missing. The combined rule of 0.1.0 counts as current while
/// it still grants the same access.
#[must_use]
pub fn rule_state() -> Rule {
    classify(fs::read_to_string(UDEV_RULE_PATH).ok().as_deref())
}

/// [`rule_state`] for the text of the installed file, if any.
fn classify(installed: Option<&str>) -> Rule {
    match installed {
        None => Rule::Missing,
        Some(text) if text == access_rule() => Rule::Current,
        Some(text) if text == UDEV_RULE_0_1_0 && rule_0_1_0_is_current() => Rule::Current,
        Some(_) => Rule::Outdated,
    }
}

/// Whether the keepalive fix is in place: the unit, and a rule that starts it. The rule
/// is the current [`keepalive_rule`] in its own file, or the line 0.1.0 put in
/// `70-8b.rules`. Only meaningful outside the sandbox, which cannot see these files.
#[must_use]
pub fn keepalive_installed() -> bool {
    fix_in_place(
        fs::read_to_string(KEEPALIVE_RULE_PATH).ok().as_deref(),
        fs::read_to_string(UDEV_RULE_PATH).ok().as_deref(),
        Path::new(KEEPALIVE_UNIT_PATH).exists(),
    )
}

/// [`keepalive_installed`] for the text of the two rule files, if any, and whether the
/// unit exists.
fn fix_in_place(fix_rule: Option<&str>, access_rule: Option<&str>, unit: bool) -> bool {
    unit && (fix_rule == Some(keepalive_rule().as_str())
        || access_rule.is_some_and(|r| r.contains("8b-keepalive@")))
}

/// Installs the access rule through `pkexec`. Blocks while the password prompt is open.
///
/// # Errors
/// Returns why the install failed, as a sentence for the window.
pub fn install_rule() -> Result<(), String> {
    pkexec(ACCESS_SCRIPT, &[&access_rule(), UDEV_RULE_PATH])
}

/// Installs the keepalive rule and unit through `pkexec`. Blocks while the password
/// prompt is open.
///
/// # Errors
/// Returns why the install failed, as a sentence for the window.
pub fn install_keepalive() -> Result<(), String> {
    pkexec(
        KEEPALIVE_SCRIPT,
        &[&keepalive_rule(), KEEPALIVE_RULE_PATH, KEEPALIVE_UNIT, KEEPALIVE_UNIT_PATH],
    )
}

/// Runs `script` as root with `args` as `$1` and up.
fn pkexec(script: &str, args: &[&str]) -> Result<(), String> {
    let out =
        Command::new("pkexec").args(["/bin/sh", "-c", script, "sh"]).args(args).output().map_err(
            |e| match e.kind() {
                std::io::ErrorKind::NotFound => "pkexec is not installed.".to_owned(),
                _ => format!("pkexec did not start: {e}."),
            },
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_0_1_0_combined_rule_reads_as_current() {
        assert_eq!(classify(Some(UDEV_RULE_0_1_0)), Rule::Current);
        assert_eq!(classify(Some(&access_rule())), Rule::Current);
    }

    #[test]
    fn the_fix_needs_its_unit_and_a_current_rule() {
        let rule = keepalive_rule();
        assert!(fix_in_place(Some(&rule), None, true));
        assert!(!fix_in_place(Some(&rule), None, false), "no unit");
        assert!(!fix_in_place(Some("KERNEL==\"event*\"\n"), Some(&access_rule()), true), "stale");
        assert!(!fix_in_place(None, None, true));
    }

    #[test]
    fn the_0_1_0_line_in_the_access_rule_counts_as_the_fix() {
        assert!(fix_in_place(None, Some(UDEV_RULE_0_1_0), true));
        assert!(!fix_in_place(None, Some(&access_rule()), true));
    }

    #[test]
    fn a_missing_or_different_file_is_not_current() {
        assert_eq!(classify(None), Rule::Missing);
        assert_eq!(classify(Some("KERNEL==\"hidraw*\"\n")), Rule::Outdated);
    }
}
