//! The udev rule that gives the user at the seat access to the config hidraw node,
//! and the systemd unit that keeps a pad in `XInput` mode connected.
//!
//! The CLI prints [`manual_command`] when opening the node is denied. The GUI
//! installs [`UDEV_RULE`] at [`UDEV_RULE_PATH`] and [`KEEPALIVE_UNIT`] at
//! [`KEEPALIVE_UNIT_PATH`] with the same reloads and trigger.

/// Where the rule goes. Numbered below 73 so systemd's `73-seat-late.rules`
/// applies the `uaccess` tag, and above 60 so `ID_INPUT_*` is already set.
pub const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-8b.rules";

/// The rule text: every hidraw node of vendor `2dc8`, plus `057e:2009` (the Pro 3 in
/// the Switch position, which also matches a genuine Nintendo Pro Controller).
///
/// The last line starts [`KEEPALIVE_UNIT`] for every `xpad` event node of vendor
/// `2dc8`. `xpad` polls a wired pad only while its event node is open, and the
/// Pro 3 in `XInput` mode resets about half a second after nothing polls it, so it
/// reconnects every 1.6 s until a program such as Steam opens it. It matches on
/// `usb_id` properties: `DRIVERS` and `ATTRS{idVendor}` sit on different parents,
/// and udev needs every parent key of a rule to match the same parent.
pub const UDEV_RULE: &str = concat!(
    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="2dc8", TAG+="uaccess""#,
    "\n",
    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="057e", ATTRS{idProduct}=="2009", TAG+="uaccess""#,
    "\n",
    r#"KERNEL=="event*", ENV{ID_USB_DRIVER}=="xpad", ENV{ID_VENDOR_ID}=="2dc8", TAG+="systemd", ENV{SYSTEMD_WANTS}+="8b-keepalive@%k.service""#,
    "\n",
);

/// Where the keepalive unit goes.
pub const KEEPALIVE_UNIT_PATH: &str = "/etc/systemd/system/8b-keepalive@.service";

/// A template unit that holds `/dev/input/<instance>` open until the pad goes away.
/// It reads without a grab, so games still see every event.
pub const KEEPALIVE_UNIT: &str = "\
[Unit]
Description=Keep xpad polling the 8BitDo controller on /dev/input/%I
BindsTo=dev-input-%i.device
After=dev-input-%i.device

[Service]
ExecStart=/bin/cat /dev/input/%I
StandardOutput=null
";

/// `text` as one single-quoted shell argument per line, each with a leading space.
fn quoted_lines(text: &str) -> String {
    text.lines().flat_map(|line| [" '", line, "'"]).collect()
}

/// One shell line that installs [`UDEV_RULE`] and [`KEEPALIVE_UNIT`] with `sudo`,
/// reloads systemd and the rules, and re-applies them to present hidraw nodes, so
/// no replug is needed.
#[must_use]
pub fn manual_command() -> String {
    format!(
        "printf '%s\\n'{rule} | sudo tee {UDEV_RULE_PATH} >/dev/null \
         && printf '%s\\n'{unit} | sudo tee {KEEPALIVE_UNIT_PATH} >/dev/null \
         && sudo systemctl daemon-reload \
         && sudo udevadm control --reload-rules \
         && sudo udevadm trigger --subsystem-match=hidraw --action=change",
        rule = quoted_lines(UDEV_RULE),
        unit = quoted_lines(KEEPALIVE_UNIT),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_command_installs_both_files() {
        // A single quote would end the shell argument early.
        assert!(!UDEV_RULE.contains('\'') && !KEEPALIVE_UNIT.contains('\''));
        let cmd = manual_command();
        for line in UDEV_RULE.lines().chain(KEEPALIVE_UNIT.lines()) {
            assert!(cmd.contains(&format!("'{line}'")), "missing: {line}");
        }
        assert!(cmd.contains(&format!("sudo tee {UDEV_RULE_PATH} ")));
        assert!(cmd.contains(&format!("sudo tee {KEEPALIVE_UNIT_PATH} ")));
    }
}
