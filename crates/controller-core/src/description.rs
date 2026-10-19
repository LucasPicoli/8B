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
use crate::model::{Mode, PROFILE_FIELDS};
use crate::view::View;

mod firmware;
mod lock;
mod prose;

pub use firmware::{firmware_allows, FirmwareVersion};
pub use lock::{Lock, LockedPort, Relock, TimedPacket, Unlock, MAX_PACKET_LEN};
pub use prose::Prose;

/// The output that turns a button off. Valid for every remappable button in every
/// mode, so no description lists it.
pub const DISABLED_OUTPUT: &str = "disabled";

/// A button entry that matches no known output. The pad still fires whatever its
/// bits say, so a recompile keeps the raw bytes. Read-only: no remap may target it.
pub const UNRECOGNISED_OUTPUT: &str = "unrecognised";

/// Everything the app knows about one controller model, apart from protocol bytes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerDescription {
    /// Model name shown to the user.
    pub display_name: String,
    /// Short model name for tight places, such as the controller picker.
    pub short_name: String,
    /// Model ids the `START_CONFIG` reply carries for this controller. Empty for a
    /// model that its config ports name, which no other model may list.
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
    /// The user may rename a profile. True when the JSON omits it.
    #[serde(default = "yes")]
    pub profile_rename: bool,
    /// What a macro step stores of each stick.
    #[serde(default)]
    pub macro_sticks: MacroSticks,
    /// The unlock and relock sequences of a controller that hides its config port.
    /// `None` when the config port always shows.
    #[serde(default)]
    pub lock: Option<Lock>,
    /// Physical buttons, in display order.
    pub buttons: Vec<Button>,
    /// The settings tabs, in display order. Each names its values by JSON pointer into
    /// the profile.
    pub settings: Vec<SettingsPage>,
    /// Drawings of the controller, in display order.
    pub views: Vec<View>,
}

/// One settings tab, such as Sticks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPage {
    /// Tab id, unique per mode. Two pages may share an id when their modes differ, as
    /// the analog and the threshold Triggers tabs do.
    pub id: String,
    /// Tab name shown to the user.
    pub label: String,
    /// The modes that show this tab. Empty means every mode.
    #[serde(default)]
    pub modes: Vec<Mode>,
    /// The sentence above the frames.
    pub lead: String,
    /// The frames of the tab, in display order.
    pub frames: Vec<SettingsFrame>,
}

impl SettingsPage {
    /// The JSON pointer of every value on this tab: the slider ends, then the flags.
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        let sliders = self.frames.iter().flat_map(|f| &f.sliders);
        let numbers = sliders.flat_map(|s| std::iter::once(&s.low).chain(&s.high));
        let flags = self.frames.iter().flat_map(|f| &f.flags).map(|f| f.field.as_str());
        numbers.map(|n| n.field.as_str()).chain(flags)
    }
}

/// A titled group of settings on a tab.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsFrame {
    /// Frame title.
    pub title: String,
    /// Sliders, in display order.
    #[serde(default)]
    pub sliders: Vec<Slider>,
    /// Check boxes, in display order.
    #[serde(default)]
    pub flags: Vec<Flag>,
}

/// A slider: one number, or the two ends of a range.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slider {
    /// Name shown to the user.
    pub label: String,
    /// Unit after the value, such as `%`. Empty shows `<value> of <max>`.
    #[serde(default)]
    pub unit: String,
    /// The number, or the low end of the range.
    pub low: NumberField,
    /// The high end of the range. `None` for a single number.
    #[serde(default)]
    pub high: Option<NumberField>,
    /// The macro stick whose moves stop playing when `low` is under the mode's
    /// [`ModeDescription::macro_stick_floor`]. Set on a stick's dead zone slider.
    #[serde(default)]
    pub macro_stick: Option<MacroStick>,
    /// The oldest firmware that has this slider. `None` for every firmware.
    #[serde(default)]
    pub min_firmware: Option<FirmwareVersion>,
}

/// What a macro step stores of each stick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroSticks {
    /// Any x and y from 0 to 255.
    #[default]
    Analog,
    /// The centre or one of 8 directions at full deflection: each axis 0, 128 or 255.
    Directions8,
}

/// A stick a macro step moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroStick {
    /// The left stick.
    LeftStick,
    /// The right stick.
    RightStick,
}

/// One integer setting and its allowed range.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberField {
    /// JSON pointer into the profile, such as `/sticks/left_min_pct`.
    pub field: String,
    /// Smallest allowed value.
    pub min: i32,
    /// Largest allowed value.
    pub max: i32,
    /// A name for each value from `min` to `max`, such as `Off`, `Pulse` and `Wave` for
    /// a choice. Empty means the value shows as a number.
    #[serde(default)]
    pub labels: Vec<String>,
    /// The oldest firmware that has this setting. `None` for every firmware.
    #[serde(default)]
    pub min_firmware: Option<FirmwareVersion>,
}

impl NumberField {
    /// The allowed range.
    #[must_use]
    pub const fn range(&self) -> LimitRange {
        LimitRange { min: self.min, max: self.max }
    }

    /// The name of `value`, if the field names its values.
    #[must_use]
    pub fn label(&self, value: i64) -> Option<&str> {
        let index = usize::try_from(value - i64::from(self.min)).ok()?;
        self.labels.get(index).map(String::as_str)
    }
}

/// One on-or-off setting.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    /// Name shown to the user.
    pub label: String,
    /// JSON pointer into the profile, such as `/sticks/invert_left_x`.
    pub field: String,
    /// Flags of the same mode that may not be on together with this one.
    #[serde(default)]
    pub excludes: Vec<String>,
    /// The oldest firmware that has this flag. `None` for every firmware.
    #[serde(default)]
    pub min_firmware: Option<FirmwareVersion>,
}

/// One mode of the controller.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeDescription {
    /// The mode.
    pub id: Mode,
    /// Name shown to the user, such as `XInput`.
    pub label: String,
    /// How the triggers read in this mode, and so in a macro step.
    pub trigger_kind: TriggerKind,
    /// Outputs a button may map to in this mode beyond the buttons and [`DISABLED_OUTPUT`].
    pub extra_outputs: Vec<ExtraOutput>,
    /// The lowest dead zone low end, in percent, at which a macro still moves a stick in
    /// this mode, read from each slider with a [`Slider::macro_stick`]. `None` when every
    /// low end plays.
    #[serde(default)]
    pub macro_stick_floor: Option<i32>,
    /// The byte the pad stores and selects this mode by. `None` for a pad whose blob
    /// does not name its mode that way.
    #[serde(default)]
    pub gamepad_mode: Option<u8>,
}

/// How a mode's triggers read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    /// A pressure from 0 to 255.
    Analog,
    /// Pressed or released. A macro step stores 255 or 0.
    OnOff,
}

/// A mode-specific output that is not a button press.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtraOutput {
    /// Output id as written in `button_mappings.target`.
    pub id: String,
    /// Name shown to the user.
    pub label: String,
    /// The buttons that may take this output. Empty means every remappable button.
    #[serde(default)]
    pub buttons: Vec<String>,
    /// The oldest firmware that has this output. `None` for every firmware.
    #[serde(default)]
    pub min_firmware: Option<FirmwareVersion>,
    /// How to use this output on the pad, shown beside a button that has it. Empty for
    /// none.
    #[serde(default)]
    pub hint: String,
    /// A warning shown when a button the controller holds on this output is set to
    /// another one. Empty for none.
    #[serde(default)]
    pub leaving_hint: String,
    /// The code a button map entry holds for this output, in hex such as `"0x400000"`.
    /// `None` for a pad that stores no codes.
    #[serde(default, deserialize_with = "hex_u32_opt")]
    pub code: Option<u32>,
}

impl ExtraOutput {
    /// Whether the button `source` may take this output.
    #[must_use]
    pub fn allows(&self, source: &str) -> bool {
        self.buttons.is_empty() || self.buttons.iter().any(|b| b == source)
    }
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

/// Name lengths and macro limits the editor offers.
///
/// Each model's tests pin them to its codec and to the profile and macro schemas. The
/// ranges of settings sit on their [`NumberField`]s. An optional limit the JSON omits
/// is a feature the controller lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Profile name length in characters.
    pub profile_name_length: LimitRange,
    /// Macro name length in characters. `None` when a macro has no name.
    pub macro_name_length: Option<LimitRange>,
    /// Number of steps in one macro, as the pad stores them.
    pub macro_steps: LimitRange,
    /// Length of one macro step in milliseconds.
    pub macro_step_ms: LimitRange,
    /// Number of plays per trigger press. The editor also offers
    /// [`REPEAT_CONTINUOUS`](crate::model::REPEAT_CONTINUOUS), which plays until the
    /// next press. `None` when a macro plays once per press.
    pub macro_repeat_count: Option<LimitRange>,
    /// Pause between two plays in milliseconds. `None` when a macro has no pause.
    pub macro_interval_ms: Option<LimitRange>,
    /// Length of each short step a stick or trigger glide becomes on write, in
    /// milliseconds. `None` when a macro step cannot glide.
    pub glide_step_ms: Option<u16>,
    /// The most trigger buttons one macro takes. `None` for no limit.
    pub triggers_per_macro: Option<u8>,
}

impl Limits {
    /// Every range the description sets, with its field name.
    fn named(&self) -> impl Iterator<Item = (&'static str, LimitRange)> {
        [
            ("profile_name_length", Some(self.profile_name_length)),
            ("macro_name_length", self.macro_name_length),
            ("macro_steps", Some(self.macro_steps)),
            ("macro_step_ms", Some(self.macro_step_ms)),
            ("macro_repeat_count", self.macro_repeat_count),
            ("macro_interval_ms", self.macro_interval_ms),
        ]
        .into_iter()
        .filter_map(|(name, range)| Some((name, range?)))
    }
}

/// One physical button.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct Button {
    /// Canonical position name, as in `button_mappings.source` and `.target`. Never shown.
    pub id: String,
    /// Name shown to the user, one per mode.
    pub labels: BTreeMap<Mode, String>,
    /// What a screen reader says for a label that is a symbol, such as "Minus" for "−".
    /// Sparse: a mode without an entry is read from its label.
    #[serde(default)]
    pub spoken_labels: BTreeMap<Mode, String>,
    /// The name printed on the physical button, the same in every mode. Set it only
    /// when the print differs from `labels`: the button column, the views and the macro
    /// editor then show it, and `labels` keeps naming the output in each mode.
    #[serde(default)]
    pub printed_label: Option<String>,
    /// A button mapping may give this button an output other than its own press.
    pub can_be_remapped: bool,
    /// This button's press may be the output of another button. A button can
    /// always map back to itself.
    pub can_be_output: bool,
    /// Pressing this button may start a macro (a macro trigger button). The same in
    /// every mode.
    #[serde(default)]
    pub can_start_macro: bool,
    /// This button's press and release may appear in a macro step (a step button). The
    /// same in every mode.
    #[serde(default)]
    pub can_be_in_macro_step: bool,
    /// A macro step holds this button's press in its left or right trigger value, as a
    /// pressure in an analog mode, not among its pressed buttons. `None` for a button.
    #[serde(default)]
    pub step_trigger_value: Option<TriggerSide>,
    /// Where this button's entry sits in the pad's button map. `None` for a pad whose
    /// map is not a list of entries.
    #[serde(default)]
    pub map_index: Option<u8>,
    /// The code a button map entry holds when it sends this button's press, one per
    /// mode, in hex such as `"0x2000"`. A mode without an entry has no such output. A
    /// code can mean another button in another mode: in `XInput`, `0x2000` is Xbox A.
    #[serde(default, deserialize_with = "hex_u32_map")]
    pub map_codes: BTreeMap<Mode, u32>,
    /// The code a macro record holds for this button, the same in every mode. It is not
    /// derived from `map_codes`: a pad may use different codes in the two places.
    #[serde(default, deserialize_with = "hex_u32_opt")]
    pub macro_code: Option<u32>,
}

/// One of the two trigger values of a macro step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSide {
    /// `MacroStep::trigger_left`.
    Left,
    /// `MacroStep::trigger_right`.
    Right,
}

impl Button {
    /// The name of this physical button in `mode`: its printed label, else its label,
    /// else its id.
    #[must_use]
    pub fn printed(&self, mode: Mode) -> &str {
        self.printed_label
            .as_deref()
            .or_else(|| self.labels.get(&mode).map(String::as_str))
            .unwrap_or(self.id.as_str())
    }

    /// What a screen reader says for this button in `mode`: its spoken label, else its
    /// label, else its id.
    #[must_use]
    pub fn spoken_label(&self, mode: Mode) -> &str {
        self.spoken_labels
            .get(&mode)
            .or_else(|| self.labels.get(&mode))
            .map_or(self.id.as_str(), String::as_str)
    }

    /// What a screen reader says for this physical button in `mode`: its printed label,
    /// read as the spoken label of a mode whose label matches it (a printed "−" reads
    /// "Minus"), else its spoken label.
    #[must_use]
    pub fn printed_spoken(&self, mode: Mode) -> &str {
        let Some(printed) = self.printed_label.as_deref() else {
            return self.spoken_label(mode);
        };
        self.labels
            .iter()
            .find(|(_, label)| *label == printed)
            .and_then(|(m, _)| self.spoken_labels.get(m))
            .map_or(printed, String::as_str)
    }
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

    /// The name of `mode` shown to the user, such as `XInput`, or its id when this
    /// controller lacks it.
    #[must_use]
    pub fn mode_label(&self, mode: Mode) -> String {
        self.mode(mode).map_or_else(|| mode.to_string(), |m| m.label.clone())
    }

    /// The settings tabs `mode` shows, in display order.
    pub fn pages(&self, mode: Mode) -> impl Iterator<Item = &SettingsPage> {
        self.settings.iter().filter(move |p| p.modes.is_empty() || p.modes.contains(&mode))
    }

    /// The number setting at JSON pointer `field` in `mode`, if one of its tabs has it.
    #[must_use]
    pub fn number(&self, mode: Mode, field: &str) -> Option<&NumberField> {
        self.pages(mode)
            .flat_map(|p| &p.frames)
            .flat_map(|f| &f.sliders)
            .flat_map(|s| std::iter::once(&s.low).chain(&s.high))
            .find(|n| n.field == field)
    }

    /// The flag at JSON pointer `field` in `mode`, if one of its tabs has it.
    #[must_use]
    pub fn flag(&self, mode: Mode, field: &str) -> Option<&Flag> {
        self.flags(mode).find(|f| f.field == field)
    }

    /// Every flag `mode` shows.
    pub fn flags(&self, mode: Mode) -> impl Iterator<Item = &Flag> {
        self.pages(mode).flat_map(|p| &p.frames).flat_map(|f| &f.flags)
    }

    /// The flags of `mode` that may not be on together with the flag at `field`, both
    /// the ones it lists and the ones that list it.
    #[must_use]
    pub fn excluded_by(&self, mode: Mode, field: &str) -> Vec<&str> {
        let listed = self.flag(mode, field).into_iter().flat_map(|f| &f.excludes);
        let listing = self.flags(mode).filter(|f| f.excludes.iter().any(|e| e == field));
        listed.map(String::as_str).chain(listing.map(|f| f.field.as_str())).collect()
    }

    /// The button with canonical name `id`.
    #[must_use]
    pub fn button(&self, id: &str) -> Option<&Button> {
        self.buttons.iter().find(|b| b.id == id)
    }

    /// Checks a button mapping from `source` to `target` in `mode`.
    ///
    /// A button may always map back to itself, even when it cannot be another
    /// button's output.
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
        let extra = mode_entry.extra_outputs.iter().find(|o| o.id == target);
        if target == DISABLED_OUTPUT
            || target == source
            || self.button(target).is_some_and(|b| b.can_be_output)
            || extra.is_some_and(|o| o.allows(source))
        {
            return Ok(());
        }
        if let Some(extra) = extra {
            return Err(Error::Validation(format!(
                "Target '{target}' is only valid for {}.",
                extra.buttons.join(", ")
            )));
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
            .chain(
                mode_entry.extra_outputs.iter().filter(|o| o.allows(source)).map(|o| o.id.as_str()),
            )
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
        if self.slot_count == 0 {
            return Err("slot_count must be above 0".to_owned());
        }
        if let Some(port) = self.config_ports.iter().find(|p| !modes.contains(&p.mode)) {
            return Err(format!("config port for unlisted mode '{}'", port.mode));
        }
        let mut usb_ids = BTreeSet::new();
        if let Some(port) =
            self.config_ports.iter().find(|p| !usb_ids.insert((p.usb.vendor, p.usb.product)))
        {
            return Err(format!(
                "config ports list USB id {:04x}:{:04x} twice: one id stands for one mode",
                port.usb.vendor, port.usb.product
            ));
        }
        for port in &self.config_ports {
            let Some(via) = port.write_via else { continue };
            let takes_writes =
                self.config_ports.iter().any(|p| p.mode == via && p.write_via.is_none());
            if via == port.mode || !takes_writes {
                return Err(format!(
                    "write_via '{via}' of the {} port must name another port that takes writes",
                    port.mode
                ));
            }
        }
        if let Some((name, _)) = self.limits.named().find(|(_, r)| r.min > r.max) {
            return Err(format!("limit '{name}' has min above max"));
        }
        if self.limits.glide_step_ms == Some(0) || self.limits.triggers_per_macro == Some(0) {
            return Err(
                "limits 'glide_step_ms' and 'triggers_per_macro' must be above 0".to_owned()
            );
        }
        if let Some(lock) = &self.lock {
            lock.check()?;
            let usb = lock.locked.usb;
            if self.config_ports.iter().any(|p| p.usb == usb) {
                return Err("lock: the locked USB id is a config port".to_owned());
            }
        }
        self.check_settings(&modes)?;
        let mut ids = BTreeSet::new();
        for button in &self.buttons {
            if !ids.insert(button.id.as_str()) {
                return Err(format!("button '{}' is listed twice", button.id));
            }
            if !button.labels.keys().copied().eq(modes.iter().copied()) {
                return Err(format!("button '{}' needs one label per listed mode", button.id));
            }
            if !button.spoken_labels.keys().all(|m| button.labels.contains_key(m)) {
                return Err(format!(
                    "button '{}' has a spoken label for a mode it lacks a label in",
                    button.id
                ));
            }
        }
        self.check_macro_buttons()?;
        let mut indexes = BTreeSet::new();
        if let Some(i) =
            self.buttons.iter().filter_map(|b| b.map_index).find(|i| !indexes.insert(*i))
        {
            return Err(format!("map_index {i} is used twice"));
        }
        let mut bytes = BTreeSet::new();
        if let Some(b) =
            self.modes.iter().filter_map(|m| m.gamepad_mode).find(|b| !bytes.insert(*b))
        {
            return Err(format!("gamepad_mode {b} is used twice"));
        }
        for mode in &self.modes {
            let mut outputs = BTreeSet::new();
            for output in &mode.extra_outputs {
                if output.id == DISABLED_OUTPUT
                    || output.id == UNRECOGNISED_OUTPUT
                    || ids.contains(output.id.as_str())
                    || !outputs.insert(output.id.as_str())
                {
                    return Err(format!(
                        "extra output '{}' clashes in {} mode",
                        output.id, mode.id
                    ));
                }
                let remappable = |id: &String| self.button(id).is_some_and(|b| b.can_be_remapped);
                if let Some(id) = output.buttons.iter().find(|id| !remappable(id)) {
                    return Err(format!(
                        "extra output '{}' names '{id}', which is no remappable button",
                        output.id
                    ));
                }
            }
        }
        self.check_views(&ids)
    }

    /// Macro button flags need macro slots, and macro slots need both flags somewhere.
    fn check_macro_buttons(&self) -> std::result::Result<(), String> {
        let starts = self.buttons.iter().any(|b| b.can_start_macro);
        let in_step = self.buttons.iter().any(|b| b.can_be_in_macro_step);
        if self.macro_slot_count == 0 && (starts || in_step) {
            return Err("macro button flags need macro_slot_count above 0".to_owned());
        }
        if self.macro_slot_count > 0 && !(starts && in_step) {
            return Err("a controller with macros needs a button that can start a macro \
                        and a button that can be in a macro step"
                .to_owned());
        }
        for side in [TriggerSide::Left, TriggerSide::Right] {
            let holders: Vec<_> =
                self.buttons.iter().filter(|b| b.step_trigger_value == Some(side)).collect();
            if holders.len() > 1 || holders.iter().any(|b| !b.can_be_in_macro_step) {
                return Err(format!(
                    "step_trigger_value {side:?} needs one button that can be in a macro step"
                ));
            }
        }
        Ok(())
    }

    /// Every tab names listed modes and shows once per mode. Every setting is a new JSON
    /// pointer into a settings group, a number's min is not above its max, and each flag
    /// a flag excludes is a flag of the same mode.
    fn check_settings(&self, modes: &BTreeSet<Mode>) -> std::result::Result<(), String> {
        for page in &self.settings {
            if let Some(mode) = page.modes.iter().find(|m| !modes.contains(m)) {
                return Err(format!("settings tab '{}' names unlisted mode '{mode}'", page.id));
            }
        }
        for &mode in modes {
            let mut tabs = BTreeSet::new();
            if let Some(page) = self.pages(mode).find(|p| !tabs.insert(p.id.as_str())) {
                return Err(format!("settings tab '{}' shows twice in {mode} mode", page.id));
            }
            let mut fields = BTreeSet::new();
            let numbers = self.pages(mode).flat_map(|p| &p.frames).flat_map(|f| &f.sliders);
            for number in numbers.flat_map(|s| std::iter::once(&s.low).chain(&s.high)) {
                if number.min > number.max {
                    return Err(format!("setting '{}' has min above max", number.field));
                }
                let values = i64::from(number.max) - i64::from(number.min) + 1;
                if !number.labels.is_empty() && i64::try_from(number.labels.len()) != Ok(values) {
                    return Err(format!(
                        "setting '{}' needs one label per value, {values} in all",
                        number.field
                    ));
                }
                if !is_settings_pointer(&number.field) || !fields.insert(number.field.as_str()) {
                    return Err(format!(
                        "setting '{}' in {mode} mode is not a new JSON pointer",
                        number.field
                    ));
                }
            }
            for flag in self.flags(mode) {
                if !is_settings_pointer(&flag.field) || !fields.insert(flag.field.as_str()) {
                    return Err(format!(
                        "setting '{}' in {mode} mode is not a new JSON pointer",
                        flag.field
                    ));
                }
            }
            for flag in self.flags(mode) {
                if let Some(e) = flag.excludes.iter().find(|e| self.flag(mode, e).is_none()) {
                    return Err(format!(
                        "flag '{}' excludes '{e}', no flag in {mode} mode",
                        flag.field
                    ));
                }
            }
        }
        Ok(())
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

/// Whether `pointer` is a JSON pointer into a settings group, not into a field every
/// profile has, such as `/name`.
fn is_settings_pointer(pointer: &str) -> bool {
    pointer
        .strip_prefix('/')
        .and_then(|path| path.split('/').next())
        .is_some_and(|group| !group.is_empty() && !PROFILE_FIELDS.contains(&group))
}

/// The default of a flag that is on unless the JSON says otherwise.
const fn yes() -> bool {
    true
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

/// Parses `"0x2000"` into a `u32`.
fn parse_hex_u32(s: &str) -> Option<u32> {
    u32::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

fn hex_u32_opt<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<u32>, D::Error> {
    Option::<String>::deserialize(d)?
        .map(|s| parse_hex_u32(&s).ok_or_else(|| bad_hex(&s)))
        .transpose()
}

fn hex_u32_map<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<Mode, u32>, D::Error> {
    BTreeMap::<Mode, String>::deserialize(d)?
        .into_iter()
        .map(|(m, s)| Ok((m, parse_hex_u32(&s).ok_or_else(|| bad_hex(&s))?)))
        .collect()
}

fn hex_u16_list<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<u16>, D::Error> {
    Vec::<String>::deserialize(d)?.iter().map(|s| parse_hex(s).ok_or_else(|| bad_hex(s))).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    use crate::devices::pro3::{DINPUT, SWITCH, XINPUT};

    pub(super) fn minimal() -> Value {
        let range = json!({ "min": 0, "max": 1 });
        json!({
            "display_name": "Test pad",
            "short_name": "Pad",
            "model_ids": ["0x6009"],
            "config_ports": [
                { "usb": { "vendor": "0x2dc8", "product": "0x310b" },
                  "mode": "xinput", "interface": 2, "framing": "plain" }
            ],
            "modes": [
                { "id": "xinput", "label": "XInput", "trigger_kind": "analog",
                  "extra_outputs": [] },
                { "id": "switch", "label": "Switch", "trigger_kind": "on_off",
                  "extra_outputs": [{ "id": "screenshot", "label": "Screenshot" }] }
            ],
            "slot_count": 3,
            "macro_slot_count": 4,
            "limits": {
                "profile_name_length": range, "macro_name_length": range, "macro_steps": range,
                "macro_step_ms": { "min": 0, "max": 65535 },
                "macro_repeat_count": { "min": 1, "max": 99 },
                "macro_interval_ms": { "min": 0, "max": 65535 },
                "glide_step_ms": 20
            },
            "settings": [
                { "id": "sticks", "label": "Sticks", "lead": "L",
                  "frames": [{ "title": "Left",
                    "sliders": [{ "label": "Dead zone", "unit": "%",
                      "low": { "field": "/sticks/min", "min": 0, "max": 90 },
                      "high": { "field": "/sticks/max", "min": 10, "max": 100 } }],
                    "flags": [
                      { "label": "Invert", "field": "/sticks/invert" },
                      { "label": "Swap", "field": "/sticks/swap", "excludes": ["/sticks/invert"] }
                    ] }] },
                { "id": "triggers", "label": "Triggers", "modes": ["switch"], "lead": "T",
                  "frames": [{ "title": "Left",
                    "sliders": [{ "label": "Press", "low": { "field": "/t", "min": 0, "max": 9 } }] }] }
            ],
            "buttons": [
                { "id": "a", "labels": { "xinput": "A", "switch": "B" },
                  "can_be_remapped": true, "can_be_output": true,
                  "can_start_macro": true, "can_be_in_macro_step": true },
                { "id": "home", "labels": { "xinput": "Home", "switch": "Home" },
                  "can_be_remapped": false, "can_be_output": true },
                { "id": "paddle", "labels": { "xinput": "P", "switch": "P" },
                  "can_be_remapped": true, "can_be_output": false }
            ],
            "views": [
                { "id": "front", "label": "Front", "svg": "front.svg" },
                { "id": "back", "label": "Back", "svg": "back.svg" }
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

    pub(super) fn parse(v: &Value) -> Result<ControllerDescription> {
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
    fn settings_follow_the_mode_and_exclusions_go_both_ways() {
        let d = parse(&minimal()).unwrap();
        let tabs = |m| d.pages(m).map(|p| p.id.as_str()).collect::<Vec<_>>();
        assert_eq!(tabs(XINPUT), ["sticks"]);
        assert_eq!(tabs(SWITCH), ["sticks", "triggers"]);
        assert_eq!(d.mode(XINPUT).map(|m| m.trigger_kind), Some(TriggerKind::Analog));
        assert_eq!(d.mode(SWITCH).map(|m| m.trigger_kind), Some(TriggerKind::OnOff));
        assert_eq!(
            d.number(SWITCH, "/t").map(NumberField::range),
            Some(LimitRange { min: 0, max: 9 })
        );
        assert!(d.number(XINPUT, "/t").is_none(), "the press point is Switch only");
        assert!(d.flag(XINPUT, "/sticks/min").is_none(), "a number is no flag");
        assert_eq!(d.excluded_by(XINPUT, "/sticks/swap"), ["/sticks/invert"]);
        assert_eq!(d.excluded_by(XINPUT, "/sticks/invert"), ["/sticks/swap"]);
    }

    #[test]
    fn semantic_check_catches_each_rule() {
        let cases: [fn(&mut Value); 34] = [
            |v| v["modes"][1]["extra_outputs"][0]["buttons"] = json!(["home"]),
            |v| v["modes"][1]["extra_outputs"][0]["buttons"] = json!(["nothing"]),
            |v| v["modes"][1]["extra_outputs"][0]["min_firmware"] = json!("1.3"),
            |v| v["limits"]["triggers_per_macro"] = json!(0),
            |v| v["macro_sticks"] = json!("directions4"),
            |v| {
                v["lock"] = json!({
                    "locked": { "usb": { "vendor": "0x2dc8", "product": "0x310b" }, "product": "P" },
                    "unlock": { "interface": 0, "endpoint": "0x02", "timeout_ms": 1,
                                "packets": [{ "at_ms": 0, "data": "00" }] },
                    "relock": { "packets": [{ "at_ms": 0, "data": "00" }] }
                });
            },
            |v| v["buttons"][1]["step_trigger_value"] = json!("left"),
            |v| {
                v["buttons"][0]["step_trigger_value"] = json!("right");
                v["buttons"][1]["can_be_in_macro_step"] = json!(true);
                v["buttons"][1]["step_trigger_value"] = json!("right");
            },
            |v| v["settings"][1]["frames"][0]["sliders"][0]["low"]["field"] = json!("/name"),
            |v| v["settings"][0]["frames"][0]["flags"][0]["field"] = json!("/mode"),
            |v| v["settings"][1]["modes"] = json!(["dinput"]),
            |v| v["settings"][1]["id"] = json!("sticks"),
            |v| v["settings"][0]["frames"][0]["sliders"][0]["low"]["min"] = json!(95),
            |v| v["settings"][0]["frames"][0]["flags"][0]["field"] = json!("/sticks/min"),
            |v| v["settings"][0]["frames"][0]["flags"][1]["excludes"] = json!(["/t"]),
            |v| v["config_ports"][0]["write_via"] = json!("xinput"),
            |v| v["config_ports"][0]["write_via"] = json!("switch"),
            |v| v["buttons"][1]["id"] = json!("a"),
            |v| {
                v["buttons"][0]["labels"].as_object_mut().unwrap().remove("switch");
            },
            |v| v["buttons"][0]["spoken_labels"] = json!({ "dinput": "Ay" }),
            |v| v["limits"]["macro_steps"] = json!({ "min": 2, "max": 1 }),
            |v| v["limits"]["macro_repeat_count"] = json!({ "min": 99, "max": 1 }),
            |v| v["limits"]["glide_step_ms"] = json!(0),
            |v| v["modes"][0]["trigger_kind"] = json!("pressure"),
            |v| v["macro_slot_count"] = json!(0),
            |v| v["buttons"][0]["can_start_macro"] = json!(false),
            |v| v["buttons"][0]["can_be_in_macro_step"] = json!(false),
            |v| v["config_ports"][0]["mode"] = json!("dinput"),
            |v| v["modes"][1]["extra_outputs"][0]["id"] = json!("a"),
            |v| v["modes"][1]["id"] = json!("xinput"),
            |v| v["views"][1]["id"] = json!("front"),
            |v| v["views"][1]["svg"] = json!("missing.svg"),
            |v| {
                v["settings"][0]["frames"][0]["sliders"][0]["low"]["labels"] = json!(["Off"]);
            },
            |v| {
                let port = v["config_ports"][0].clone();
                let mut second = port;
                second["mode"] = json!("switch");
                v["config_ports"].as_array_mut().unwrap().push(second);
            },
        ];
        for (i, break_it) in cases.iter().enumerate() {
            let mut v = minimal();
            break_it(&mut v);
            assert!(parse(&v).is_err(), "case {i} should fail");
        }
    }

    /// `minimal` with every field a Pro 3 description leaves out.
    fn every_new_field() -> Value {
        let mut v = minimal();
        v["profile_rename"] = json!(false);
        v["macro_sticks"] = json!("directions8");
        let limits = v["limits"].as_object_mut().unwrap();
        for absent in
            ["macro_name_length", "macro_repeat_count", "macro_interval_ms", "glide_step_ms"]
        {
            limits.remove(absent);
        }
        limits.insert("triggers_per_macro".into(), json!(1));
        v["modes"][1]["extra_outputs"][0]["buttons"] = json!(["paddle"]);
        v["modes"][1]["extra_outputs"][0]["min_firmware"] = json!("1.03");
        let frame = &mut v["settings"][0]["frames"][0];
        frame["flags"][0]["min_firmware"] = json!("1.03");
        frame["sliders"][0]["min_firmware"] = json!("1.04");
        frame["sliders"][0]["high"]["min_firmware"] = json!("1.05");
        v["lock"] = lock::tests::a_lock();
        v
    }

    #[test]
    fn every_new_field_matches_the_schema() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../schemas/controller-description-v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::draft202012::new(&schema).unwrap();
        for v in [minimal(), every_new_field()] {
            let errors: Vec<String> = validator.iter_errors(&v).map(|e| e.to_string()).collect();
            assert!(errors.is_empty(), "{errors:?}");
        }
        let mut odd = every_new_field();
        odd["lock"]["unlock"]["packets"][0]["data"] = json!("0008000");
        assert!(!validator.is_valid(&odd), "the schema refuses odd hex too");
    }

    #[test]
    fn every_new_field_reads_back() {
        let d = parse(&every_new_field()).unwrap();
        let gate = |s| FirmwareVersion::parse(s);
        assert!(!d.profile_rename);
        assert_eq!(d.macro_sticks, MacroSticks::Directions8);
        let l = d.limits;
        assert_eq!(
            (l.macro_name_length, l.macro_repeat_count, l.macro_interval_ms, l.glide_step_ms),
            (None, None, None, None),
            "absent: not offered"
        );
        assert_eq!(l.triggers_per_macro, Some(1));
        let screenshot = &d.mode(SWITCH).unwrap().extra_outputs[0];
        assert_eq!(
            (screenshot.buttons.as_slice(), screenshot.min_firmware),
            (&["paddle".to_owned()][..], gate("1.03"))
        );
        assert_eq!(d.flag(XINPUT, "/sticks/invert").unwrap().min_firmware, gate("1.03"));
        let slider = &d.settings[0].frames[0].sliders[0];
        assert_eq!(slider.min_firmware, gate("1.04"));
        assert_eq!(d.number(XINPUT, "/sticks/max").unwrap().min_firmware, gate("1.05"));
        assert_eq!(d.number(XINPUT, "/sticks/min").unwrap().min_firmware, None);
        assert_eq!(d.lock.unwrap().relock.packets.len(), 2);
    }

    #[test]
    fn a_pro_3_style_description_keeps_every_default() {
        let d = parse(&minimal()).unwrap();
        assert!(d.profile_rename);
        assert_eq!(d.macro_sticks, MacroSticks::Analog);
        assert_eq!(d.limits.glide_step_ms, Some(20));
        assert_eq!(d.limits.triggers_per_macro, None);
        assert!(d.lock.is_none());
        let screenshot = &d.mode(SWITCH).unwrap().extra_outputs[0];
        assert!(screenshot.buttons.is_empty() && screenshot.min_firmware.is_none());
        assert!(screenshot.allows("a") && screenshot.allows("paddle"));
    }

    #[test]
    fn an_extra_output_goes_only_on_its_buttons_and_in_its_modes() {
        let d = parse(&every_new_field()).unwrap();
        assert!(d.validate_remap(SWITCH, "paddle", "screenshot").is_ok());
        assert!(matches!(
            d.validate_remap(SWITCH, "a", "screenshot"),
            Err(Error::Validation(m)) if m == "Target 'screenshot' is only valid for paddle."
        ));
        assert!(matches!(
            d.validate_remap(XINPUT, "paddle", "screenshot"),
            Err(Error::Validation(m)) if m == "Target 'screenshot' is only valid for switch mode."
        ));
        assert!(matches!(
            d.validate_remap(SWITCH, "a", "nothing"),
            Err(Error::Validation(m)) if m == "Invalid remap target 'nothing'. Valid targets: a, home, disabled."
        ));
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
        assert!(d.validate_remap(XINPUT, "paddle", "a").is_ok());
        assert!(d.validate_remap(XINPUT, "a", "home").is_ok());
        assert!(d.validate_remap(XINPUT, "a", DISABLED_OUTPUT).is_ok());
        assert!(d.validate_remap(XINPUT, "a", UNRECOGNISED_OUTPUT).is_err(), "read-only");
        assert!(d.validate_remap(SWITCH, "a", "screenshot").is_ok());
        assert!(d.validate_remap(XINPUT, "home", "a").is_err());
        assert!(d.validate_remap(XINPUT, "a", "paddle").is_err());
        assert!(d.validate_remap(DINPUT, "a", "a").is_err(), "unlisted mode");
        assert!(matches!(
            d.validate_remap(XINPUT, "a", "screenshot"),
            Err(Error::Validation(m)) if m == "Target 'screenshot' is only valid for switch mode."
        ));
    }
}
