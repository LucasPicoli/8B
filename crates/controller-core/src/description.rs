//! Controller description: the per-model facts the UI and the remap check read.
//!
//! Each controller model embeds one `description.json` and one SVG per [`View`].
//! [`ControllerDescription::parse`] reads the JSON with unknown fields denied, attaches
//! the SVGs, then runs a semantic check. The format is
//! documented for hand editing in `schemas/controller-description-v1.schema.json`.
//! Protocol bytes stay on [`crate::device::ControllerSpec`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer};

use crate::device::ConfigPort;
use crate::error::{Error, Result};
use crate::model::Mode;
use crate::view::View;

/// The output that turns a button off. Valid for every remappable button in every
/// mode, so no description lists it.
pub const DISABLED_OUTPUT: &str = "disabled";

/// Everything the app knows about one controller model, apart from protocol bytes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerDescription {
    /// Model name shown to the user.
    pub display_name: String,
    /// Model ids the `START_CONFIG` reply carries for this controller.
    #[serde(deserialize_with = "hex_u16_list")]
    pub model_ids: Vec<u16>,
    /// The config interface of every current mode, one per USB id.
    pub config_ports: Vec<ConfigPort>,
    /// Supported modes, in display order.
    pub modes: Vec<ModeDescription>,
    /// Number of profile slots per mode.
    pub slot_count: u8,
    /// Number of macro slots per profile slot.
    pub macro_slot_count: u8,
    /// Value ranges the editor offers.
    pub limits: Limits,
    /// Physical buttons, in display order.
    pub buttons: Vec<Button>,
    /// Drawings of the controller, in display order.
    pub views: Vec<View>,
}

/// One mode of the controller.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeDescription {
    /// The mode.
    pub id: Mode,
    /// How the triggers are tuned in this mode.
    pub trigger_kind: TriggerKind,
    /// Outputs a button may map to in this mode beyond the buttons and [`DISABLED_OUTPUT`].
    pub extra_outputs: Vec<ExtraOutput>,
}

/// How the triggers of a mode are tuned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerKind {
    /// Analog triggers with a min and max range.
    Analog,
    /// Digital triggers with a press threshold.
    Threshold,
}

/// A mode-specific output that is not a button press.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtraOutput {
    /// Output id as written in `button_mappings.target`.
    pub id: String,
    /// Name shown to the user.
    pub label: String,
}

/// An inclusive range of allowed values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitRange {
    /// Smallest allowed value.
    pub min: i32,
    /// Largest allowed value.
    pub max: i32,
}

/// Value ranges the editor offers. A unit test pins them to the profile and macro schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Profile name length in characters.
    pub profile_name_length: LimitRange,
    /// Macro name length in characters.
    pub macro_name_length: LimitRange,
    /// Number of steps in one macro.
    pub macro_steps: LimitRange,
    /// Stick dead zone (`*_min_pct`).
    pub stick_min_pct: LimitRange,
    /// Stick outer range (`*_max_pct`).
    pub stick_max_pct: LimitRange,
    /// Analog trigger min and max.
    pub trigger_pct: LimitRange,
    /// Threshold trigger press point.
    pub trigger_threshold_pct: LimitRange,
    /// Vibration strength level.
    pub vibration_level: LimitRange,
}

impl Limits {
    /// Every range with its field name.
    const fn named(&self) -> [(&'static str, LimitRange); 8] {
        [
            ("profile_name_length", self.profile_name_length),
            ("macro_name_length", self.macro_name_length),
            ("macro_steps", self.macro_steps),
            ("stick_min_pct", self.stick_min_pct),
            ("stick_max_pct", self.stick_max_pct),
            ("trigger_pct", self.trigger_pct),
            ("trigger_threshold_pct", self.trigger_threshold_pct),
            ("vibration_level", self.vibration_level),
        ]
    }
}

/// One physical button.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Button {
    /// Canonical position name, as in `button_mappings.source` and `.target`. Never shown.
    pub id: String,
    /// Name shown to the user, one per mode.
    pub labels: BTreeMap<Mode, String>,
    /// A button mapping may give this button an output other than its own press.
    pub can_be_remapped: bool,
    /// This button's press may be the output of another button.
    pub can_be_output: bool,
}

impl ControllerDescription {
    /// Parses a description, attaches its view SVGs from `svgs` (file name, contents),
    /// and checks it.
    ///
    /// # Errors
    /// Returns [`Error::Decode`] if the JSON does not match the format, names an SVG
    /// missing from `svgs`, or fails the semantic check.
    pub fn parse(json: &str, svgs: &[(&str, &'static str)]) -> Result<Self> {
        let mut description: Self = serde_json::from_str(json).map_err(|e| invalid(&e))?;
        for view in &mut description.views {
            let Some((_, data)) = svgs.iter().find(|(name, _)| *name == view.svg) else {
                return Err(invalid(&format!("view '{}' needs '{}' embedded", view.id, view.svg)));
            };
            view.attach(data).map_err(|e| invalid(&e))?;
        }
        description.check().map_err(|e| invalid(&e))?;
        Ok(description)
    }

    /// The entry for `mode`, if this controller has it.
    #[must_use]
    pub fn mode(&self, mode: Mode) -> Option<&ModeDescription> {
        self.modes.iter().find(|m| m.id == mode)
    }

    /// The button with canonical name `id`.
    #[must_use]
    pub fn button(&self, id: &str) -> Option<&Button> {
        self.buttons.iter().find(|b| b.id == id)
    }

    /// Checks a button mapping from `source` to `target` in `mode`.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] with a message fit to show to the user.
    pub fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()> {
        let Some(mode_entry) = self.mode(mode) else {
            return Err(Error::Validation(format!("This controller has no {mode} mode.")));
        };
        let Some(button) = self.button(source) else {
            let names: Vec<&str> = self.buttons.iter().map(|b| b.id.as_str()).collect();
            return Err(Error::Validation(format!(
                "Invalid source control '{source}'. Valid names: {}.",
                names.join(", ")
            )));
        };
        if !button.can_be_remapped {
            return Err(Error::Validation(format!(
                "Cannot remap '{source}': that button cannot be remapped."
            )));
        }
        if target == DISABLED_OUTPUT
            || self.button(target).is_some_and(|b| b.can_be_output)
            || mode_entry.extra_outputs.iter().any(|o| o.id == target)
        {
            return Ok(());
        }
        let elsewhere: Vec<&str> = self
            .modes
            .iter()
            .filter(|m| m.extra_outputs.iter().any(|o| o.id == target))
            .map(|m| m.id.as_str())
            .collect();
        if !elsewhere.is_empty() {
            return Err(Error::Validation(format!(
                "Target '{target}' is only valid for {} mode.",
                elsewhere.join(" and ")
            )));
        }
        let outputs: Vec<&str> = self
            .buttons
            .iter()
            .filter(|b| b.can_be_output)
            .map(|b| b.id.as_str())
            .chain(std::iter::once(DISABLED_OUTPUT))
            .chain(mode_entry.extra_outputs.iter().map(|o| o.id.as_str()))
            .collect();
        Err(Error::Validation(format!(
            "Invalid remap target '{target}'. Valid targets: {}.",
            outputs.join(", ")
        )))
    }

    /// The semantic rules serde cannot express.
    fn check(&self) -> std::result::Result<(), String> {
        let modes: BTreeSet<Mode> = self.modes.iter().map(|m| m.id).collect();
        if modes.is_empty() || modes.len() != self.modes.len() {
            return Err("modes must be listed once each, at least one".to_owned());
        }
        if self.model_ids.is_empty() || self.slot_count == 0 {
            return Err("model_ids and slot_count must not be empty".to_owned());
        }
        if let Some(port) = self.config_ports.iter().find(|p| !modes.contains(&p.mode)) {
            return Err(format!("config port for unlisted mode '{}'", port.mode));
        }
        if let Some((name, _)) = self.limits.named().into_iter().find(|(_, r)| r.min > r.max) {
            return Err(format!("limit '{name}' has min above max"));
        }
        let mut ids = BTreeSet::new();
        for button in &self.buttons {
            if !ids.insert(button.id.as_str()) {
                return Err(format!("button '{}' is listed twice", button.id));
            }
            if !button.labels.keys().copied().eq(modes.iter().copied()) {
                return Err(format!("button '{}' needs one label per listed mode", button.id));
            }
        }
        for mode in &self.modes {
            let mut outputs = BTreeSet::new();
            for output in &mode.extra_outputs {
                if output.id == DISABLED_OUTPUT
                    || ids.contains(output.id.as_str())
                    || !outputs.insert(output.id.as_str())
                {
                    return Err(format!(
                        "extra output '{}' clashes in {} mode",
                        output.id, mode.id
                    ));
                }
            }
        }
        self.check_views(&ids)
    }

    /// Every hotspot names a button, at most once per view, and every button has a
    /// hotspot in at least one view.
    fn check_views(&self, button_ids: &BTreeSet<&str>) -> std::result::Result<(), String> {
        let mut view_ids = BTreeSet::new();
        let mut placed = BTreeSet::new();
        for view in &self.views {
            if !view_ids.insert(view.id.as_str()) {
                return Err(format!("view '{}' is listed twice", view.id));
            }
            let mut in_view = BTreeSet::new();
            for hotspot in &view.hotspots {
                let button = hotspot.button.as_str();
                if !button_ids.contains(button) || !in_view.insert(button) {
                    return Err(format!(
                        "'{}' marks '{button}', which is no button or is marked twice",
                        view.svg
                    ));
                }
                placed.insert(button);
            }
        }
        button_ids
            .iter()
            .find(|id| !placed.contains(*id))
            .map_or(Ok(()), |id| Err(format!("button '{id}' has no hotspot")))
    }
}

fn invalid(e: &dyn std::fmt::Display) -> Error {
    Error::Decode(format!("controller description: {e}"))
}

/// Parses `"0x2dc8"`.
fn parse_hex(s: &str) -> Option<u16> {
    u16::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

fn bad_hex<E: serde::de::Error>(s: &str) -> E {
    E::custom(format!("expected a hex id like \"0x2dc8\", got \"{s}\""))
}

/// Deserializes a `"0x2dc8"` string into a `u16`.
pub(crate) fn hex_u16<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u16, D::Error> {
    let s = String::deserialize(d)?;
    parse_hex(&s).ok_or_else(|| bad_hex(&s))
}

fn hex_u16_list<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<u16>, D::Error> {
    Vec::<String>::deserialize(d)?.iter().map(|s| parse_hex(s).ok_or_else(|| bad_hex(s))).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn minimal() -> Value {
        let range = json!({ "min": 0, "max": 1 });
        json!({
            "display_name": "Test pad",
            "model_ids": ["0x6009"],
            "config_ports": [
                { "usb": { "vendor": "0x2dc8", "product": "0x310b" },
                  "mode": "xinput", "interface": 2, "framing": "plain" }
            ],
            "modes": [
                { "id": "xinput", "trigger_kind": "analog", "extra_outputs": [] },
                { "id": "switch", "trigger_kind": "threshold",
                  "extra_outputs": [{ "id": "screenshot", "label": "Screenshot" }] }
            ],
            "slot_count": 3,
            "macro_slot_count": 4,
            "limits": {
                "profile_name_length": range, "macro_name_length": range,
                "macro_steps": range, "stick_min_pct": range, "stick_max_pct": range,
                "trigger_pct": range, "trigger_threshold_pct": range, "vibration_level": range
            },
            "buttons": [
                { "id": "a", "labels": { "xinput": "A", "switch": "B" },
                  "can_be_remapped": true, "can_be_output": true },
                { "id": "home", "labels": { "xinput": "Home", "switch": "Home" },
                  "can_be_remapped": false, "can_be_output": true },
                { "id": "paddle", "labels": { "xinput": "P", "switch": "P" },
                  "can_be_remapped": true, "can_be_output": false }
            ],
            "views": [
                { "id": "front", "svg": "front.svg" },
                { "id": "back", "svg": "back.svg" }
            ]
        })
    }

    const FRONT: &str = r#"<svg viewBox="0 0 100 50">
        <circle data-button="a" cx="10" cy="10" r="5"/>
        <circle data-button="home" cx="30" cy="10" r="5"/>
    </svg>"#;

    /// `a` shows in both views, which is allowed.
    const BACK: &str = r#"<svg viewBox="0 0 100 50">
        <circle data-button="paddle" cx="10" cy="10" r="5"/>
        <circle data-button="a" cx="30" cy="10" r="5"/>
    </svg>"#;

    fn parse_with(
        v: &Value,
        front: &'static str,
        back: &'static str,
    ) -> Result<ControllerDescription> {
        ControllerDescription::parse(&v.to_string(), &[("front.svg", front), ("back.svg", back)])
    }

    fn parse(v: &Value) -> Result<ControllerDescription> {
        parse_with(v, FRONT, BACK)
    }

    #[test]
    fn minimal_parses_with_hex_ids() {
        let d = parse(&minimal()).unwrap();
        assert_eq!(d.model_ids, [0x6009]);
        assert_eq!(d.config_ports[0].usb.product, 0x310B);
        assert_eq!(d.views[1].svg_data, BACK);
        assert_eq!(d.views[1].hotspots[1].button, "a");
    }

    #[test]
    fn rejects_unknown_fields_and_bad_hex() {
        let mut v = minimal();
        v["extra"] = json!(1);
        assert!(matches!(parse(&v), Err(Error::Decode(_))));
        let mut v = minimal();
        v["model_ids"] = json!(["6009"]);
        assert!(parse(&v).is_err());
    }

    #[test]
    fn semantic_check_catches_each_rule() {
        let cases: [fn(&mut Value); 8] = [
            |v| v["buttons"][1]["id"] = json!("a"),
            |v| {
                v["buttons"][0]["labels"].as_object_mut().unwrap().remove("switch");
            },
            |v| v["limits"]["macro_steps"] = json!({ "min": 2, "max": 1 }),
            |v| v["config_ports"][0]["mode"] = json!("dinput"),
            |v| v["modes"][1]["extra_outputs"][0]["id"] = json!("a"),
            |v| v["modes"][1]["id"] = json!("xinput"),
            |v| v["views"][1]["id"] = json!("front"),
            |v| v["views"][1]["svg"] = json!("missing.svg"),
        ];
        for (i, break_it) in cases.iter().enumerate() {
            let mut v = minimal();
            break_it(&mut v);
            assert!(parse(&v).is_err(), "case {i} should fail");
        }
    }

    #[test]
    fn view_marks_must_name_buttons_once_per_view() {
        let v = minimal();
        let bad: [(&'static str, &'static str); 4] = [
            (
                r#"<svg viewBox="0 0 100 50"><circle data-button="a" cx="10" cy="10" r="5"/></svg>"#,
                BACK,
            ),
            (
                FRONT,
                r#"<svg viewBox="0 0 100 50"><circle data-button="nothing" cx="10" cy="10" r="5"/></svg>"#,
            ),
            (
                FRONT,
                r#"<svg viewBox="0 0 100 50"><circle data-button="paddle" cx="10" cy="10" r="5"/><circle data-button="paddle" cx="30" cy="10" r="5"/></svg>"#,
            ),
            (
                FRONT,
                r##"<svg viewBox="0 0 100 50"><circle data-button="paddle" cx="10" cy="10" r="5" fill="#ff0000"/></svg>"##,
            ),
        ];
        for (i, (front, back)) in bad.iter().enumerate() {
            assert!(parse_with(&v, front, back).is_err(), "case {i} should fail");
        }
        assert!(
            matches!(parse_with(&v, FRONT, r#"<svg viewBox="0 0 100 50"/>"#), Err(Error::Decode(m)) if m.contains("'paddle' has no hotspot"))
        );
    }

    #[test]
    fn remap_follows_the_flags() {
        let d = parse(&minimal()).unwrap();
        assert!(d.validate_remap(Mode::XInput, "paddle", "a").is_ok());
        assert!(d.validate_remap(Mode::XInput, "a", "home").is_ok());
        assert!(d.validate_remap(Mode::XInput, "a", DISABLED_OUTPUT).is_ok());
        assert!(d.validate_remap(Mode::Switch, "a", "screenshot").is_ok());
        assert!(d.validate_remap(Mode::XInput, "home", "a").is_err());
        assert!(d.validate_remap(Mode::XInput, "a", "paddle").is_err());
        assert!(d.validate_remap(Mode::DInput, "a", "a").is_err(), "unlisted mode");
        assert!(matches!(
            d.validate_remap(Mode::XInput, "a", "screenshot"),
            Err(Error::Validation(m)) if m == "Target 'screenshot' is only valid for switch mode."
        ));
    }
}
