//! The udev rules and the systemd unit around the config hidraw node.
//!
//! Two installs, each with its own command:
//!
//! - The access rule, [`UDEV_RULE`] at [`UDEV_RULE_PATH`], gives the user at the seat
//!   access to the hidraw node. [`manual_command`] installs it. The CLI prints that
//!   command when opening the node is denied, and the GUI installs the same file.
//! - The keepalive fix, [`keepalive_rule`] at [`KEEPALIVE_RULE_PATH`] plus
//!   [`KEEPALIVE_UNIT`] at [`KEEPALIVE_UNIT_PATH`], keeps a pad that resets without a
//!   poller connected. [`keepalive_command`] installs it. Only controllers whose
//!   description sets `needs_keepalive` on a config port get a line.

use std::fmt::Write;

use crate::description::ControllerDescription;
use crate::device::{ControllerSpec, UsbId};
use crate::devices::pro3::Pro3;

/// Where the access rule goes. Numbered below 73 so systemd's `73-seat-late.rules`
/// applies the `uaccess` tag.
pub const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-8b.rules";

/// The access rule text: every hidraw node of vendor `2dc8`, plus `057e:2009` (the
/// Pro 3 in the Switch position, which also matches a genuine Nintendo Pro Controller).
pub const UDEV_RULE: &str = concat!(
    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="2dc8", TAG+="uaccess""#,
    "\n",
    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="057e", ATTRS{idProduct}=="2009", TAG+="uaccess""#,
    "\n",
);

/// The rule file of 0.1.0, which an installed copy may still be.
///
/// It held the access lines and a keepalive line for every `xpad` event node of vendor
/// `2dc8`. It still grants access, so it counts as current. Its keepalive line and the
/// one in [`KEEPALIVE_RULE_PATH`] start the same unit twice, which systemd treats as
/// one start.
pub const UDEV_RULE_0_1_0: &str = concat!(
    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="2dc8", TAG+="uaccess""#,
    "\n",
    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="057e", ATTRS{idProduct}=="2009", TAG+="uaccess""#,
    "\n",
    r#"KERNEL=="event*", ENV{ID_USB_DRIVER}=="xpad", ENV{ID_VENDOR_ID}=="2dc8", TAG+="systemd", ENV{SYSTEMD_WANTS}+="8b-keepalive@%k.service""#,
    "\n",
);

/// Where the keepalive rule goes. Numbered below 73 like the access rule, and above
/// 60 so `ID_USB_DRIVER` and `ID_VENDOR_ID` are already set.
pub const KEEPALIVE_RULE_PATH: &str = "/etc/udev/rules.d/71-8b-keepalive.rules";

/// Where the keepalive unit goes.
pub const KEEPALIVE_UNIT_PATH: &str = "/etc/systemd/system/8b-keepalive@.service";

/// A template unit that holds `/dev/input/<instance>` open until the pad goes away.
/// It reads without a grab, so games still see every event.
pub const KEEPALIVE_UNIT: &str = "\
[Unit]
Description=Keep xpad polling the controller on /dev/input/%I
BindsTo=dev-input-%i.device
After=dev-input-%i.device

[Service]
ExecStart=/bin/cat /dev/input/%I
StandardOutput=null
";

/// The descriptions of every supported controller model. A new model goes in here.
fn descriptions() -> impl Iterator<Item = &'static ControllerDescription> {
    // A unit test loads every embedded description, so `ok()` drops nothing in a shipped build.
    Pro3.description().ok().into_iter()
}

/// The USB ids that need the keepalive in `descriptions`, each once, in the order the
/// descriptions list them.
fn keepalive_ids<'a>(
    descriptions: impl IntoIterator<Item = &'a ControllerDescription>,
) -> Vec<UsbId> {
    let mut ids = Vec::new();
    for port in descriptions.into_iter().flat_map(|d| d.config_ports.iter()) {
        if port.needs_keepalive && !ids.contains(&port.usb) {
            ids.push(port.usb);
        }
    }
    ids
}

/// The keepalive rule text for `descriptions`: one line per USB id that needs it, or an
/// empty string when none does. [`keepalive_rule`] is this over every supported model.
///
/// Each line starts [`KEEPALIVE_UNIT`] for the `xpad` event node of that id. `xpad` polls
/// a wired pad only while its event node is open, and the Pro 3 in `XInput` mode resets
/// about half a second after nothing polls it, so it reconnects every 1.6 s until a
/// program such as Steam opens it. The line matches on `usb_id` properties: `DRIVERS`
/// and `ATTRS{idVendor}` sit on different parents, and udev needs every parent key of
/// a rule to match the same parent.
#[must_use]
pub fn keepalive_rule_for<'a>(
    descriptions: impl IntoIterator<Item = &'a ControllerDescription>,
) -> String {
    keepalive_ids(descriptions).into_iter().fold(String::new(), |mut rule, id| {
        let _ = writeln!(
            rule,
            "KERNEL==\"event*\", ENV{{ID_USB_DRIVER}}==\"xpad\", ENV{{ID_VENDOR_ID}}==\"{:04x}\", \
             ENV{{ID_MODEL_ID}}==\"{:04x}\", TAG+=\"systemd\", \
             ENV{{SYSTEMD_WANTS}}+=\"8b-keepalive@%k.service\"",
            id.vendor, id.product,
        );
        rule
    })
}

/// The keepalive rule text for every supported controller model.
#[must_use]
pub fn keepalive_rule() -> String {
    keepalive_rule_for(descriptions())
}

/// Whether any supported controller needs the keepalive.
#[must_use]
pub fn keepalive_needed() -> bool {
    !keepalive_rule().is_empty()
}

/// `text` as one single-quoted shell argument per line, each with a leading space.
fn quoted_lines(text: &str) -> String {
    text.lines().flat_map(|line| [" '", line, "'"]).collect()
}

/// A shell step that writes `text` to `path` with `sudo tee`.
fn tee(text: &str, path: &str) -> String {
    format!("printf '%s\\n'{} | sudo tee {path} >/dev/null", quoted_lines(text))
}

/// One shell line that installs [`UDEV_RULE`] with `sudo`, reloads the rules, and
/// re-applies them to present hidraw nodes, so no replug is needed.
#[must_use]
pub fn manual_command() -> String {
    format!(
        "{} && sudo udevadm control --reload-rules \
         && sudo udevadm trigger --subsystem-match=hidraw --action=change",
        tee(UDEV_RULE, UDEV_RULE_PATH),
    )
}

/// One shell line that installs [`keepalive_rule`] and [`KEEPALIVE_UNIT`] with `sudo`,
/// reloads systemd and the rules, and re-applies them to present input nodes, so no
/// replug is needed.
#[must_use]
pub fn keepalive_command() -> String {
    keepalive_command_for(&keepalive_rule())
}

/// [`keepalive_command`] for the rule text `rule`.
fn keepalive_command_for(rule: &str) -> String {
    format!(
        "{} && {} && sudo systemctl daemon-reload && sudo udevadm control --reload-rules \
         && sudo udevadm trigger --subsystem-match=input --action=change",
        tee(rule, KEEPALIVE_RULE_PATH),
        tee(KEEPALIVE_UNIT, KEEPALIVE_UNIT_PATH),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A single quote would end the shell argument early.
    fn assert_quotable(text: &str) {
        assert!(!text.contains('\''), "single quote in: {text}");
    }

    #[test]
    fn access_command_carries_only_the_access_lines() {
        assert_quotable(UDEV_RULE);
        let cmd = manual_command();
        for line in UDEV_RULE.lines() {
            assert!(cmd.contains(&format!("'{line}'")), "missing: {line}");
        }
        assert!(cmd.contains(&format!("sudo tee {UDEV_RULE_PATH} ")));
        assert!(!cmd.contains("event*") && !cmd.contains("keepalive"));
        assert!(!cmd.contains("daemon-reload"));
    }

    #[test]
    fn keepalive_command_carries_only_the_keepalive_files() {
        assert_quotable(&keepalive_rule());
        assert_quotable(KEEPALIVE_UNIT);
        let cmd = keepalive_command();
        for line in keepalive_rule().lines().chain(KEEPALIVE_UNIT.lines()) {
            assert!(cmd.contains(&format!("'{line}'")), "missing: {line}");
        }
        assert!(cmd.contains(&format!("sudo tee {KEEPALIVE_RULE_PATH} ")));
        assert!(cmd.contains(&format!("sudo tee {KEEPALIVE_UNIT_PATH} ")));
        assert!(!cmd.contains("hidraw") && !cmd.contains(UDEV_RULE_PATH));
    }

    #[test]
    fn keepalive_rule_comes_from_the_description() {
        assert!(keepalive_needed());
        assert_eq!(
            keepalive_rule(),
            "KERNEL==\"event*\", ENV{ID_USB_DRIVER}==\"xpad\", ENV{ID_VENDOR_ID}==\"2dc8\", \
             ENV{ID_MODEL_ID}==\"310b\", TAG+=\"systemd\", \
             ENV{SYSTEMD_WANTS}+=\"8b-keepalive@%k.service\"\n"
        );
    }

    /// The Pro 3 description with its `XInput` port moved to `product` and, if
    /// `needs_keepalive`, flagged: stands in for a second controller model.
    fn another_model(product: u16, needs_keepalive: bool) -> ControllerDescription {
        let mut d = Pro3.description().unwrap().clone();
        for port in &mut d.config_ports {
            port.usb.vendor = 0x1234;
            port.usb.product = product;
            port.needs_keepalive = needs_keepalive;
        }
        d.config_ports.truncate(1);
        d
    }

    #[test]
    fn a_second_model_adds_its_own_line() {
        let pro3 = Pro3.description().unwrap();
        let other = another_model(0x00a1, true);
        let rule = keepalive_rule_for([pro3, &other]);
        assert_eq!(rule.lines().count(), 2);
        assert!(rule.contains("ID_MODEL_ID}==\"310b\""));
        assert!(rule.contains("ENV{ID_VENDOR_ID}==\"1234\", ENV{ID_MODEL_ID}==\"00a1\""));
        assert_quotable(&rule);
        // The command carries every line.
        let cmd = keepalive_command_for(&rule);
        for line in rule.lines() {
            assert!(cmd.contains(&format!("'{line}'")), "missing: {line}");
        }
    }

    #[test]
    fn a_model_without_the_flag_adds_nothing_and_a_repeat_adds_one_line() {
        let pro3 = Pro3.description().unwrap();
        let quiet = another_model(0x00a1, false);
        assert_eq!(keepalive_rule_for([pro3, &quiet]), keepalive_rule_for([pro3]));
        assert_eq!(keepalive_rule_for([pro3, pro3]).lines().count(), 1);
        assert_eq!(keepalive_rule_for([&quiet]), "");
    }

    #[test]
    fn the_0_1_0_rule_is_the_access_rule_plus_one_keepalive_line() {
        assert!(UDEV_RULE_0_1_0.starts_with(UDEV_RULE));
        let rest = &UDEV_RULE_0_1_0[UDEV_RULE.len()..];
        assert_eq!(rest.lines().count(), 1);
        assert!(rest.starts_with("KERNEL==\"event*\""));
    }

    #[test]
    fn rule_paths_sort_below_73() {
        for path in [UDEV_RULE_PATH, KEEPALIVE_RULE_PATH] {
            let name = path.rsplit('/').next().unwrap_or(path);
            assert!(name < "73", "{name}");
        }
    }
}
