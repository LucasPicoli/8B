//! The drop-down settings of a tab, such as what the Ultimate 2's motion sensor moves.
//! The profile holds the id of the picked option.

use std::rc::Rc;

use controller_core::description::{self as described};
use serde_json::Value;
use slint::{ModelRc, SharedString, VecModel};

use crate::language::prose;
use crate::settings::patch;
use crate::state::AppState;
use crate::ui::ChoiceRow;

impl AppState {
    /// Sets the choice at `pointer` in the selected slot to option `id`. A pointer that
    /// names no choice, or an id it lacks, changes nothing.
    pub fn set_choice(&mut self, pointer: &str, id: &str) {
        let Some((mode, _)) = self.selected_slot() else { return };
        if self.description().choice(mode, pointer).and_then(|c| c.option(id)).is_none() {
            return;
        }
        self.edit(|p| patch(p, pointer, id.into()));
    }

    /// Sets the choice at `pointer` to the option at `index`, in display order.
    pub fn set_choice_at(&mut self, pointer: &str, index: usize) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let id = self.description().choice(mode, pointer).and_then(|c| c.options.get(index));
        if let Some(id) = id.map(|o| o.id.clone()) {
            self.set_choice(pointer, &id);
        }
    }
}

/// The index of the option `json` holds at `choice`'s pointer.
fn picked(choice: &described::Choice, json: &Value) -> Option<usize> {
    let id = json.pointer(&choice.field)?.as_str()?;
    choice.options.iter().position(|o| o.id == id)
}

/// The rows of `frame`'s choices, filled from `shown`, with what differs from `pad`. A
/// choice whose value names no option is left out.
pub fn rows(
    frame: &described::SettingsFrame,
    shown: &Value,
    pad: Option<&Value>,
) -> Vec<ChoiceRow> {
    frame
        .choices
        .iter()
        .filter_map(|choice| {
            let current = picked(choice, shown)?;
            let label = |i: usize| choice.options.get(i).map(|o| prose(&o.label));
            let was = pad
                .and_then(|p| picked(choice, p))
                .filter(|&old| old != current)
                .and_then(label)
                .unwrap_or_default();
            let options: Vec<SharedString> =
                choice.options.iter().map(|o| prose(&o.label).into()).collect();
            Some(ChoiceRow {
                label: prose(&choice.label).into(),
                field: choice.field.as_str().into(),
                options: ModelRc::from(Rc::new(VecModel::from(options))),
                current: i32::try_from(current).unwrap_or_default(),
                was: was.into(),
            })
        })
        .collect()
}
