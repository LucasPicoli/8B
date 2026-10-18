//! Why a profile or a macro is not valid, as typed data.
//!
//! Core never words a reason for the screen. [`Reason`]'s `Display` is the English the
//! CLI prints, and a GUI matches each variant to a sentence of its own. A variant holds
//! ids, which the GUI looks up in the description, or the English it shows a name in.

use std::fmt;

use crate::model::Mode;

/// Why a profile or a macro is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// The profile schema refused the file. The text is the schema crate's English.
    ProfileSchema(String),
    /// The macro schema refused the file. The text is the schema crate's English.
    MacroSchema(String),
    /// The profile passed the schema but does not read as a profile. The text is the
    /// reader's English.
    NotAProfile(String),
    /// The file does not have the macro shape. The text is the reader's English.
    NotAMacro(String),
    /// A macro names its start buttons in `trigger` and in `trigger_buttons`.
    BothTriggers,
    /// A macro names no start buttons at all.
    NoTriggerButtons,
    /// A profile holds its macros in `macros` and in `macro_refs`.
    BothMacroLists,
    /// The controller has no such mode.
    NoMode(Mode),
    /// The profile's `device` or `kind` is another model's.
    WrongDevice {
        /// The file's value.
        got: String,
        /// The model's display name.
        shown: String,
        /// This model's value.
        want: String,
    },
    /// The controller has no macro slots, so it takes no macro.
    NoMacroSlots {
        /// The model's display name.
        display_name: String,
    },
    /// A macro sits in a macro slot the controller lacks.
    SlotOutOfRange {
        /// How many macro slots the controller has.
        count: u8,
    },
    /// Two macros sit in the same macro slot.
    SlotTwice {
        /// The macro slot, 0-based.
        slot: u8,
    },
    /// The profile has a settings group its mode lacks.
    UnknownGroup {
        /// The group, such as `sticks`.
        group: String,
        /// The profile's mode.
        mode: Mode,
    },
    /// An embedded macro is for another mode than its profile.
    MacroModeMismatch {
        /// The macro's mode label.
        got: String,
        /// The profile's mode label.
        want: String,
    },
    /// A macro step presses a button the target mode lacks.
    ButtonNotInMode {
        /// The button's id.
        button: String,
        /// The mode the macro is in.
        from: Mode,
        /// The target mode's label.
        mode: String,
    },
    /// A mapping's source is not a button of the controller.
    UnknownSource {
        /// The file's source id.
        source: String,
    },
    /// A mapping's target is not an output of the mode.
    UnknownTarget {
        /// The file's target id.
        target: String,
        /// The profile's mode.
        mode: Mode,
    },
    /// A mapping gives a button an output the mode keeps for other buttons.
    TargetNotOnButton {
        /// The file's source id.
        source: String,
        /// The file's target id.
        target: String,
        /// The profile's mode.
        mode: Mode,
    },
    /// One button starts two macros.
    TwoMacrosOneButton {
        /// The button's id.
        button: String,
        /// The profile's mode.
        mode: Mode,
    },
    /// A number setting is missing or outside its range.
    OutOfRange {
        /// The setting's JSON pointer, such as `/sticks/left_min_pct`.
        field: String,
        /// The profile's mode.
        mode: Mode,
        /// The lowest value allowed.
        lo: i64,
        /// The highest value allowed.
        hi: i64,
    },
    /// An on/off setting is missing or not a boolean.
    NotBool {
        /// The setting's JSON pointer.
        field: String,
        /// The profile's mode.
        mode: Mode,
    },
    /// A flag is on together with flags it excludes.
    Excludes {
        /// The flag's JSON pointer.
        flag: String,
        /// The pointers of the flags it clashes with, in description order.
        clashes: Vec<String>,
        /// The profile's mode.
        mode: Mode,
    },
    /// The macro name is empty.
    NameEmpty {
        /// The longest name, in UTF-16 units.
        max: usize,
    },
    /// The macro name is too long.
    NameTooLong {
        /// The name's length, in UTF-16 units.
        len: usize,
        /// The model's short name.
        short_name: String,
        /// The longest name, in UTF-16 units.
        max: usize,
    },
    /// The macro has more steps than the controller holds.
    TooManySteps {
        /// The macro's steps.
        needed: usize,
        /// The model's short name.
        short_name: String,
        /// The most steps the controller holds.
        max: usize,
    },
    /// The macro's glides expand to more steps than the controller holds.
    TooManyGlideSteps {
        /// The steps on the controller, glides expanded.
        needed: usize,
        /// The most steps the controller holds.
        max: usize,
    },
    /// The macro plays less than once.
    RepeatCount {
        /// The file's count.
        count: i64,
    },
    /// A step presses and releases the same button.
    PressAndRelease {
        /// The step, 1-based.
        step: usize,
        /// The button's id.
        button: String,
        /// The macro's mode.
        mode: Mode,
    },
    /// The last step glides, so there is nothing to glide to.
    GlideNoNextStep {
        /// The step, 1-based.
        step: usize,
        /// The control's English name: a frame title or a button label.
        name: String,
    },
    /// A step glides a trigger in a mode whose triggers are on or off.
    GlideOnOffTrigger {
        /// The step, 1-based.
        step: usize,
        /// The control's English name.
        name: String,
        /// The mode's label.
        mode: String,
    },
    /// A step glides a control to the value it already has.
    GlideSameValue {
        /// The step, 1-based.
        step: usize,
        /// The control's English name.
        name: String,
    },
    /// A reason found in the profile's embedded macro at `index`. Displays as `reason`.
    InMacro {
        /// The macro's index in `macros`, 0-based.
        index: usize,
        /// The reason the macro gives.
        reason: Box<Self>,
    },
}

/// The last segment of a JSON pointer: `left_min_pct` for `/sticks/left_min_pct`.
fn name(field: &str) -> &str {
    field.rsplit('/').next().unwrap_or(field)
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileSchema(text) | Self::MacroSchema(text) => f.write_str(text),
            Self::NotAProfile(e) => write!(f, "This is not a profile: {e}."),
            Self::NotAMacro(e) => write!(f, "This is not a macro: {e}."),
            Self::BothTriggers => {
                f.write_str("A macro has either trigger or trigger_buttons, not both.")
            }
            Self::NoTriggerButtons => {
                f.write_str("This is not a macro: it has no trigger_buttons.")
            }
            Self::BothMacroLists => {
                f.write_str("A profile holds its macros in macros or in macro_refs, not both.")
            }
            Self::NoMode(mode) => write!(f, "This controller has no {mode} mode."),
            Self::WrongDevice { got, shown, want } => {
                write!(f, "This profile is for '{got}', not the {shown} ('{want}').")
            }
            Self::NoMacroSlots { display_name } => write!(f, "{display_name} has no macro slots."),
            Self::SlotOutOfRange { count } => write!(
                f,
                "This controller has {count} macro slots, 0 to {}.",
                count.saturating_sub(1)
            ),
            Self::SlotTwice { slot } => write!(f, "Two macros are in macro slot {slot}."),
            Self::UnknownGroup { group, mode } => {
                write!(f, "This controller has no '{group}' settings in {mode} mode.")
            }
            Self::MacroModeMismatch { got, want } => {
                write!(f, "This macro is for {got} mode; the profile is for {want} mode.")
            }
            Self::ButtonNotInMode { button, mode, .. } => {
                write!(f, "The button '{button}' of this macro does not exist in {mode} mode.")
            }
            Self::UnknownSource { source } => {
                write!(f, "'{source}' is not a button of this controller.")
            }
            Self::UnknownTarget { target, mode } => {
                write!(f, "'{target}' is not an output in {mode} mode.")
            }
            Self::TargetNotOnButton { source, target, mode } => {
                write!(f, "'{source}' cannot take '{target}' in {mode} mode.")
            }
            Self::TwoMacrosOneButton { button, .. } => {
                write!(f, "'{button}' starts two macros. A button starts at most one macro.")
            }
            Self::OutOfRange { field, lo, hi, .. } => {
                write!(f, "{} must be {lo} to {hi}.", name(field))
            }
            Self::NotBool { field, .. } => write!(f, "{} must be true or false.", name(field)),
            Self::Excludes { flag, clashes, .. } => {
                let names: Vec<&str> = clashes.iter().map(|c| name(c)).collect();
                let joined = match names.split_last() {
                    Some((last, rest)) if !rest.is_empty() => {
                        format!("{} and {last}", rest.join(", "))
                    }
                    _ => names.concat(),
                };
                write!(f, "{} cannot be on together with {joined}.", name(flag))
            }
            Self::NameEmpty { max } => write!(f, "Give the macro a name of 1 to {max} characters."),
            Self::NameTooLong { len, short_name, max } => {
                write!(f, "The name has {len} characters; the {short_name} takes at most {max}.")
            }
            Self::TooManySteps { needed, short_name, max } => write!(
                f,
                "The macro has {needed} steps; the {short_name} holds at most {max}. Remove steps."
            ),
            Self::TooManyGlideSteps { needed, max } => write!(
                f,
                "With its glides, this macro needs {needed} steps on the controller, and the \
                 controller holds {max}. Shorten a glide or remove steps."
            ),
            Self::RepeatCount { count } => write!(f, "repeat.count must be >= 1 (got {count})."),
            Self::PressAndRelease { step, button, .. } => {
                write!(f, "Step {step}: '{button}' appears in both press and release.")
            }
            Self::GlideNoNextStep { step, name } => {
                write!(f, "Step {step}: {name} cannot glide, there is no next step.")
            }
            Self::GlideOnOffTrigger { step, name, mode } => write!(
                f,
                "Step {step}: {name} cannot glide in {mode} mode, its triggers are on or off."
            ),
            Self::GlideSameValue { step, name } => {
                write!(f, "Step {step}: {name} has the same value in the next step.")
            }
            Self::InMacro { reason, .. } => reason.fmt(f),
        }
    }
}

/// Serializes as the English `Display`, the CLI's JSON `reason`.
impl serde::Serialize for Reason {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
