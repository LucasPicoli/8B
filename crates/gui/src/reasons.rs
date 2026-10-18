//! The sentence the GUI shows for each reason core gives that a file or an edit is not
//! valid. Core's English, with the JSON pointer, goes to the log.

use controller_core::description::ControllerDescription;
use controller_core::model::Mode;
use controller_core::service::validation::ValidationError;
use controller_core::{Error, Reason};
use tr::tr;

use crate::buttons::output_label;
use crate::language::prose;
use crate::render::and_list;

/// Which value of a slider a number setting holds.
enum End {
    /// The slider has one value.
    Single,
    /// The low end of a range.
    Low,
    /// The high end of a range.
    High,
}

/// The sentences for `errors`, each once, in order. Each error's pointer and English
/// go to the log.
pub fn problems(description: &ControllerDescription, errors: &[ValidationError]) -> Vec<String> {
    let mut shown: Vec<String> = Vec::new();
    for error in errors {
        log::info!("not valid at {}: {}", error.path, error.reason);
        let sentence = worded(description, &error.reason);
        if !shown.contains(&sentence) {
            shown.push(sentence);
        }
    }
    shown
}

/// The sentence for a check that failed with `error`. Its English goes to the log.
pub fn failure(description: &ControllerDescription, error: &Error) -> String {
    log::info!("not valid: {error}");
    match error {
        Error::Invalid(reason) => worded(description, reason),
        _ => not_valid(),
    }
}

/// The sentence for a reason core gave with no pointer to name it by.
fn not_valid() -> String {
    tr!("Something in this file is not valid.")
}

/// The sentence for `reason`.
#[must_use]
pub fn worded(d: &ControllerDescription, reason: &Reason) -> String {
    let label = |mode: &Mode| d.mode_label(*mode);
    match reason {
        Reason::ProfileSchema(_) | Reason::NotAProfile(_) => {
            tr!("This file is not a valid profile.")
        }
        Reason::MacroSchema(_) | Reason::NotAMacro(_) => tr!("This file is not a valid macro."),
        Reason::BothTriggers => {
            tr!("This file is not a valid macro: it names its start buttons in two ways.")
        }
        Reason::NoTriggerButtons => {
            tr!("This file is not a valid macro: it has no list of start buttons.")
        }
        Reason::BothMacroLists => {
            tr!("This file is not a valid profile: it holds its macros in two ways.")
        }
        Reason::NoMode(mode) => tr!("This controller has no {mode} mode.", mode = label(mode)),
        Reason::WrongDevice { .. } => tr!("This profile is for another controller model."),
        Reason::NoMacroSlots { .. } => tr!("This controller has no macro slots."),
        Reason::SlotOutOfRange { count } => {
            tr!("This controller has {n} macro slot, and the profile uses one it does not have."
                | "This controller has {n} macro slots, and the profile uses one it does not have."
                    % *count)
        }
        Reason::SlotTwice { slot } => {
            tr!("Two macros are in Macro {n}.", n = u16::from(*slot) + 1)
        }
        Reason::UnknownGroup { mode, .. } => {
            tr!("This profile has settings that {mode} mode does not have.", mode = label(mode))
        }
        Reason::MacroModeMismatch { got, want } => tr!(
            "This macro is for {got} mode, and the profile is for {want} mode.",
            got = got,
            want = want
        ),
        Reason::ButtonNotInMode { button, from, mode } => tr!(
            "The macro uses {button}, which {mode} mode does not have.",
            button = output_label(d, *from, button),
            mode = mode
        ),
        Reason::UnknownSource { .. } => {
            tr!("The profile remaps a button this controller does not have.")
        }
        Reason::UnknownTarget { mode, .. } => {
            tr!("The profile uses an output that {mode} mode does not have.", mode = label(mode))
        }
        Reason::TargetNotOnButton { source, target, mode } => tr!(
            "{button} cannot take {output}.",
            button = output_label(d, *mode, source),
            output = output_label(d, *mode, target)
        ),
        Reason::TwoMacrosOneButton { button, mode } => tr!(
            "{button} starts two macros. A button starts at most one macro.",
            button = output_label(d, *mode, button)
        ),
        Reason::OutOfRange { field, mode, lo, hi } => out_of_range(d, *mode, field, *lo, *hi),
        Reason::NotBool { field, mode } => {
            let Some((frame, flag)) = flag(d, *mode, field) else { return not_valid() };
            tr!("{frame}, “{flag}” must be on or off.", frame = frame, flag = flag)
        }
        Reason::Excludes { flag, clashes, mode } => excludes(d, *mode, flag, clashes),
        Reason::NameEmpty { max } => tr!("Give the macro a name of at most {n} character."
            | "Give the macro a name of at most {n} characters." % *max),
        Reason::NameTooLong { .. }
        | Reason::TooManySteps { .. }
        | Reason::TooManyGlideSteps { .. } => too_big(reason),
        Reason::RepeatCount { .. } => tr!("The macro must play at least once."),
        Reason::PressAndRelease { step, button, mode } => tr!(
            "Step {n}: {button} appears in both press and release.",
            n = step,
            button = output_label(d, *mode, button)
        ),
        Reason::GlideNoNextStep { step, name } => tr!(
            "Step {n}: {name} cannot glide, because there is no next step.",
            n = step,
            name = prose(name)
        ),
        Reason::GlideOnOffTrigger { step, name, mode } => tr!(
            "Step {n}: {name} cannot glide in {mode} mode, where triggers are only on or off.",
            n = step,
            name = prose(name),
            mode = mode
        ),
        Reason::GlideSameValue { step, name } => tr!(
            "Step {n}: {name} has the same value in the next step.",
            n = step,
            name = prose(name)
        ),
        Reason::InMacro { index, reason } => {
            tr!("Macro {n}: {reason}", n = index.saturating_add(1), reason = worded(d, reason))
        }
    }
}

/// The two sentences for a macro name or step count over the controller's limit.
fn too_big(reason: &Reason) -> String {
    match reason {
        Reason::NameTooLong { len, max, .. } => format!(
            "{} {}",
            tr!("The name is {n} character long." | "The name is {n} characters long." % *len),
            tr!("This controller takes at most {n} character."
                | "This controller takes at most {n} characters." % *max)
        ),
        Reason::TooManySteps { needed, max, .. } => format!(
            "{} {}",
            tr!("The macro has {n} step." | "The macro has {n} steps." % *needed),
            tr!("This controller holds at most {n} step. Remove steps."
                | "This controller holds at most {n} steps. Remove steps." % *max)
        ),
        Reason::TooManyGlideSteps { needed, max } => format!(
            "{} {}",
            tr!("With its glides, this macro needs {n} step on the controller."
                | "With its glides, this macro needs {n} steps on the controller." % *needed),
            tr!("This controller holds at most {n} step. Shorten a glide or remove steps."
                | "This controller holds at most {n} steps. Shorten a glide or remove steps."
                    % *max)
        ),
        _ => String::new(),
    }
}

/// The sentence for number setting `field` in `mode` outside `lo` to `hi`.
fn out_of_range(d: &ControllerDescription, mode: Mode, field: &str, lo: i64, hi: i64) -> String {
    let Some((frame, slider, end)) = slider(d, mode, field) else { return not_valid() };
    match end {
        End::Single => tr!(
            "{frame}, {slider} must be {lo} to {hi}.",
            frame = frame,
            slider = slider,
            lo = lo,
            hi = hi
        ),
        End::Low => tr!(
            "{frame}, {slider}: the low end must be {lo} to {hi}.",
            frame = frame,
            slider = slider,
            lo = lo,
            hi = hi
        ),
        End::High => tr!(
            "{frame}, {slider}: the high end must be {lo} to {hi}.",
            frame = frame,
            slider = slider,
            lo = lo,
            hi = hi
        ),
    }
}

/// The sentence for flag `field` in `mode` on together with the flags `clashes`.
fn excludes(d: &ControllerDescription, mode: Mode, field: &str, clashes: &[String]) -> String {
    let Some((frame, label)) = flag(d, mode, field) else { return not_valid() };
    let others: Vec<String> = clashes
        .iter()
        .filter_map(|c| flag(d, mode, c))
        .map(|(_, other)| tr!("“{label}”", label = other))
        .collect();
    tr!(
        "{frame}, “{flag}” cannot be on together with {list}.",
        frame = frame,
        flag = label,
        list = and_list(&others)
    )
}

/// The frame title and slider label of number setting `field` in `mode`, and which
/// value of the slider it is.
fn slider(d: &ControllerDescription, mode: Mode, field: &str) -> Option<(String, String, End)> {
    d.pages(mode).flat_map(|p| &p.frames).find_map(|frame| {
        frame.sliders.iter().find_map(|s| {
            let end = match &s.high {
                None if s.low.field == field => End::Single,
                Some(_) if s.low.field == field => End::Low,
                Some(high) if high.field == field => End::High,
                _ => return None,
            };
            Some((prose(&frame.title), prose(&s.label), end))
        })
    })
}

/// The frame title and label of on/off setting `field` in `mode`.
fn flag(d: &ControllerDescription, mode: Mode, field: &str) -> Option<(String, String)> {
    d.pages(mode).flat_map(|p| &p.frames).find_map(|frame| {
        let flag = frame.flags.iter().find(|f| f.field == field)?;
        Some((prose(&frame.title), prose(&flag.label)))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use controller_core::device::ControllerSpec as _;
    use controller_core::devices::pro3::{Pro3, XINPUT};

    use super::*;

    fn d() -> &'static ControllerDescription {
        Pro3.description().unwrap()
    }

    #[test]
    fn a_setting_is_named_by_its_frame_and_slider() {
        let reason = Reason::OutOfRange {
            field: "/sticks/left_min_pct".into(),
            mode: XINPUT,
            lo: 0,
            hi: 90,
        };
        assert_eq!(worded(d(), &reason), "Left stick, Dead zone: the low end must be 0 to 90.");
    }

    #[test]
    fn an_embedded_macro_reason_names_its_macro_and_the_button_label() {
        let inner = Reason::PressAndRelease { step: 3, button: "right face".into(), mode: XINPUT };
        let reason = Reason::InMacro { index: 0, reason: Box::new(inner) };
        assert_eq!(worded(d(), &reason), "Macro 1: Step 3: B appears in both press and release.");
    }

    #[test]
    fn a_count_takes_its_plural_form() {
        let reason = Reason::NameTooLong { len: 1, short_name: "Pro 3".into(), max: 16 };
        assert_eq!(
            worded(d(), &reason),
            "The name is 1 character long. This controller takes at most 16 characters."
        );
        let reason = Reason::NoMode(XINPUT);
        assert_eq!(worded(d(), &reason), "This controller has no XInput mode.");
    }
}
