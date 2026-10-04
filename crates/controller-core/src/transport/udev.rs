//! The udev rule that gives the user at the seat access to the config hidraw node.
//!
//! The CLI prints [`UDEV_MANUAL_COMMAND`] when opening the node is denied. The GUI
//! installs [`UDEV_RULE`] at [`UDEV_RULE_PATH`] with the same reload and trigger.

/// Where the rule goes. Numbered below 73 so systemd's `73-seat-late.rules`
/// applies the `uaccess` tag.
pub const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-8b.rules";

/// The rule text: every hidraw node of vendor `2dc8`, plus `057e:2009` (the Pro 3 in
/// the Switch position, which also matches a genuine Nintendo Pro Controller).
pub const UDEV_RULE: &str = concat!(
    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="2dc8", TAG+="uaccess""#,
    "\n",
    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="057e", ATTRS{idProduct}=="2009", TAG+="uaccess""#,
    "\n",
);

/// One shell line that installs [`UDEV_RULE`] at [`UDEV_RULE_PATH`] with `sudo`,
/// reloads the rules and re-applies them to present hidraw nodes, so no replug is
/// needed.
pub const UDEV_MANUAL_COMMAND: &str = concat!(
    "printf '%s\\n' ",
    r#"'KERNEL=="hidraw*", ATTRS{idVendor}=="2dc8", TAG+="uaccess"' "#,
    r#"'KERNEL=="hidraw*", ATTRS{idVendor}=="057e", ATTRS{idProduct}=="2009", TAG+="uaccess"'"#,
    " | sudo tee /etc/udev/rules.d/70-8b.rules >/dev/null",
    " && sudo udevadm control --reload-rules",
    " && sudo udevadm trigger --subsystem-match=hidraw --action=change",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_command_installs_the_rule() {
        for line in UDEV_RULE.lines() {
            assert!(UDEV_MANUAL_COMMAND.contains(&format!("'{line}'")), "missing: {line}");
        }
        assert!(UDEV_MANUAL_COMMAND.contains(&format!("sudo tee {UDEV_RULE_PATH} ")));
    }
}
