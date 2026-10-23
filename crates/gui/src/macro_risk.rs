//! The bar over a macro the controller lost, or in a macro slot it erases on its own.
//! Pure: no Slint, no I/O. The write still goes ahead: this only warns.

use controller_core::service::validation::at_risk;
use tr::tr;

use crate::state::AppState;

/// The bar of macro slot `n` of the selected slot: its title and body when the
/// controller lost the steps of the macro there, or erases that macro slot on its own.
/// Nothing otherwise. A macro edited into the macro slot is no longer the lost one.
#[must_use]
pub fn risk_bar(state: &AppState, n: u8) -> Option<[String; 2]> {
    let (mode, number) = state.selected_slot()?;
    let description = state.description();
    let slot = state.slot(mode, number);
    let in_risky_slot = at_risk(description, mode, number, n);
    let lost = !slot.macro_unsaved(n)
        && slot.pad_macros.iter().any(|m| m.lost && m.def.macro_slot == Some(n));
    let shown = u16::from(n) + 1;
    let label = description.mode_label(mode);
    match (lost, in_risky_slot) {
        (true, false) => Some([
            tr!("The controller's firmware erased macro {n}.", n = shown),
            tr!("Import it again, or delete it."),
        ]),
        (true, true) => Some([
            tr!("A firmware bug erased macro {n}.", n = shown),
            tr!(
                "The bug erases this macro slot in profile {slot} of {mode} mode. Import the macro into another macro slot, or delete it.",
                slot = number,
                mode = label
            ),
        ]),
        (false, true) => Some([
            tr!(
                "A firmware bug erases this macro slot in profile {slot} of {mode} mode.",
                slot = number,
                mode = label
            ),
            tr!("8B cannot prevent this. Use another macro slot, or keep a copy."),
        ]),
        (false, false) => None,
    }
}
