//! Macro slots at risk: pages the controller erases on its own.
//!
//! Some pads erase the steps of a macro slot by themselves when they start in some mode.
//! The description lists those macro slots per mode
//! ([`ModeDescription::macro_slots_at_risk`](crate::description::ModeDescription)). The
//! write still goes ahead: this only warns.

use crate::description::ControllerDescription;
use crate::model::{MacroDefinition, Mode};

/// Whether the controller erases macro slot `macro_slot` (from 0) of profile slot `slot`
/// on its own in `mode`.
#[must_use]
pub fn at_risk(description: &ControllerDescription, mode: Mode, slot: u8, macro_slot: u8) -> bool {
    description.mode(mode).is_some_and(|m| {
        m.macro_slots_at_risk.iter().any(|at| at.slot == slot && at.macro_slot == macro_slot)
    })
}

/// One warning per macro of `macros` that sits in a macro slot at risk in profile slot
/// `slot` of `mode`, see [`at_risk`].
#[must_use]
pub fn macro_risk_warnings(
    description: &ControllerDescription,
    mode: Mode,
    slot: u8,
    macros: &[MacroDefinition],
) -> Vec<String> {
    macros
        .iter()
        .filter(|m| m.macro_slot.is_some_and(|n| at_risk(description, mode, slot, n)))
        .map(|m| {
            format!(
                "Macro '{}' is in macro slot {} of profile {slot}. A firmware bug erases that \
                 macro slot in {} mode. Use another macro slot, or keep a copy.",
                m.name,
                m.macro_slot.map_or(0, |n| u16::from(n) + 1),
                description.mode_label(mode)
            )
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::device::ControllerSpec as _;
    use crate::devices::pro3::{DINPUT, XINPUT};
    use crate::devices::ultimate2::Ultimate2;
    use crate::model::MacroStep;

    /// A one-step macro named `name` in macro slot `macro_slot` of `mode`.
    fn in_macro_slot(name: &str, mode: Mode, macro_slot: u8) -> MacroDefinition {
        MacroDefinition {
            name: name.to_owned(),
            mode,
            trigger_buttons: Vec::new(),
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps: vec![MacroStep::default()],
            macro_slot: Some(macro_slot),
        }
    }

    #[test]
    fn the_ultimate_2_erases_dinput_slot_3_macro_slot_2_only() {
        let d = Ultimate2.description().unwrap();
        assert!(at_risk(d, DINPUT, 3, 1));
        assert!(!at_risk(d, DINPUT, 3, 0), "macro slot 1 holds the pad's own byte, not erased");
        assert!(!at_risk(d, DINPUT, 2, 1), "another profile slot");
        assert!(!at_risk(d, XINPUT, 3, 1), "another mode");
    }

    #[test]
    fn a_macro_in_a_macro_slot_at_risk_gets_one_warning_naming_it() {
        let d = Ultimate2.description().unwrap();
        let macros = [in_macro_slot("Safe", DINPUT, 0), in_macro_slot("Combo", DINPUT, 1)];
        let warnings = macro_risk_warnings(d, DINPUT, 3, &macros);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let warning = warnings.first().unwrap();
        assert!(warning.contains("'Combo'") && warning.contains("DInput"), "{warning}");
        assert_eq!(macro_risk_warnings(d, DINPUT, 2, &macros), Vec::<String>::new());
    }
}
