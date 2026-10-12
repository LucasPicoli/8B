//! Put and delete one macro, and plan the macro edits of a write job.
//!
//! [`ProfileWriteOrchestrator::put_macro`] and [`ProfileWriteOrchestrator::delete_macro`]
//! are one-job [`ProfileWriteOrchestrator::write_slots`] runs. The batch plans each job's
//! macro edits here: the new descriptors go in the bank, and each put adds a step page
//! that [`super::pages`] writes after it.

use serde_json::Value;

use super::batch::{Bank, MacroEdit, WriteJob, WriteOp};
use super::pages::{OldMacro, Page};
use super::write::{failure_from, Plan, ProfileWriteOrchestrator};
use crate::error::{Error, ErrorCategory, Result};
use crate::model::glide::{expand, fold};
use crate::model::macros::{macro_from_json, macro_to_json};
use crate::model::{
    CanonicalProfile, MacroDefinition, MacroSlot, Mode, RawProfilePayload, Slot, WriteResult,
};
use crate::protocol::wire_write::MACRO_CHUNK_LEN;
use crate::service::validation::validate_macro;
use crate::service::{ConfirmPolicy, ReadbackResult};

/// Success message of a put.
const PUT: &str = "Macro written.";
/// Success message of a delete.
const DELETED: &str = "Macro deleted.";
/// Success message of a job with several macro edits.
const EDITED: &str = "Macros written.";

impl ProfileWriteOrchestrator<'_> {
    /// Puts the macro in `macro_json` into macro slot `macro_slot` of `slot` in `mode`,
    /// creating it or replacing the macro there. The slot's other macros, its profile and
    /// the other slots stay as read.
    ///
    /// The macro is validated against the model first and must be for `mode`, and the
    /// profile slot must hold a profile. `policy` is asked only when the macro slot
    /// already holds a macro.
    #[must_use]
    pub fn put_macro(
        &self,
        macro_json: &Value,
        mode: Mode,
        slot: Slot,
        macro_slot: MacroSlot,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        match self.check_macro(macro_json, mode, macro_slot) {
            Ok(definition) => {
                self.edit_macro(mode, slot, MacroEdit::Put { macro_slot, definition }, policy)
            }
            Err(e) => failure_from(mode, slot, &e),
        }
    }

    /// Removes the macro in macro slot `macro_slot` of `slot` in `mode`. Only its
    /// descriptor is zeroed; the step page stays. An empty macro slot succeeds without a
    /// write. `policy` is asked before the macro goes.
    #[must_use]
    pub fn delete_macro(
        &self,
        mode: Mode,
        slot: Slot,
        macro_slot: MacroSlot,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        self.edit_macro(mode, slot, MacroEdit::Delete { macro_slot }, policy)
    }

    /// One job that makes `edit` and keeps the slot's profile.
    fn edit_macro(
        &self,
        mode: Mode,
        slot: Slot,
        edit: MacroEdit,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        let op =
            WriteOp::Upload { profile: None, drop_macros: Vec::new(), macro_edits: vec![edit] };
        self.write_slots(&[WriteJob { mode, slot, op }], policy).pop().unwrap_or_else(|| {
            WriteResult::failure(mode, slot, ErrorCategory::None, "Nothing was written.")
        })
    }

    /// Validates `macro_json` for the model and `mode` and reads it. No device access.
    pub(super) fn check_macro(
        &self,
        macro_json: &Value,
        mode: Mode,
        macro_slot: MacroSlot,
    ) -> Result<MacroDefinition> {
        self.check_macro_slot(macro_slot)?;
        let errors = validate_macro(self.model, macro_json)?;
        if !errors.is_empty() {
            let details: Vec<String> =
                errors.iter().map(|e| format!("{}: {}", e.path, e.reason)).collect();
            return Err(Error::Validation(format!(
                "Macro validation failed: {}",
                details.join("; ")
            )));
        }
        let def = macro_from_json(macro_json)?;
        if def.mode != mode {
            return Err(Error::Validation(format!(
                "Mode mismatch: the target mode is '{mode}' but the macro is for '{}'.",
                def.mode
            )));
        }
        Ok(def)
    }

    /// Refuses a macro slot past the model's macro slot count. No device access.
    fn check_macro_slot(&self, macro_slot: MacroSlot) -> Result<()> {
        let description = self.model.description()?;
        let count = description.macro_slot_count;
        if macro_slot.get() < count {
            return Ok(());
        }
        Err(Error::Validation(match count {
            0 => format!("The {} has no macro slots.", description.short_name),
            _ => format!(
                "Macro slot {} does not exist: the {} has macro slots 0 to {}.",
                macro_slot.get(),
                description.short_name,
                count - 1
            ),
        }))
    }

    /// Plans the macro edits of `job` into `bank`: the descriptors in its blob, the
    /// pages of the puts with the old steps of each replace read as backups.
    ///
    /// `rb` is the slot as read, and `carries_profile` says the job writes a profile
    /// into it. `policy` is asked before a macro is replaced or removed. Returns the
    /// job's message: `Write` when the bank changed, `Nothing` when it did not, empty
    /// for a job with no macro edits.
    pub(super) fn plan_macros(
        &self,
        bank: &mut Bank,
        job: &WriteJob,
        rb: &ReadbackResult,
        carries_profile: bool,
        policy: &ConfirmPolicy,
    ) -> Result<Plan<String>> {
        let (mode, slot) = (job.mode, job.slot);
        let edits = match &job.op {
            WriteOp::Upload { macro_edits, .. } => macro_edits.as_slice(),
            WriteOp::Clear => &[],
        };
        let Some(first) = edits.first() else { return Ok(Plan::Nothing(String::new())) };
        if !rb.slot_active && !carries_profile {
            return Err(Error::Validation(format!(
                "Write a profile to slot {} first.",
                slot.get()
            )));
        }
        // An empty slot's descriptors are leftovers, not macros.
        let held = if rb.slot_active {
            self.model.decode_macro_metadata(&bank.backup, slot)?
        } else {
            Vec::new()
        };
        let held_in = |m: MacroSlot| held.iter().find(|h| h.macro_slot == Some(m.get()));
        let step_ms = self.model.description()?.limits.glide_step_ms;
        let mut blob = bank.blob.clone();
        let mut changed = false;
        for (i, edit) in edits.iter().enumerate() {
            let m = edit.macro_slot();
            if edits.iter().take(i).any(|e| e.macro_slot() == m) {
                return Err(Error::Validation(format!(
                    "The job edits macro slot {} twice.",
                    m.get()
                )));
            }
            self.check_macro_slot(m)?;
            let old = held_in(m);
            let (warning, action, def) = match (edit, old) {
                (MacroEdit::Put { definition, .. }, old) => {
                    self.check_definition(definition, mode, m)?;
                    let warning = old.map(|old| {
                        format!(
                            "Macro slot {} of slot {} in {mode} mode already holds the macro '{}'.",
                            m.get(),
                            slot.get(),
                            old.name
                        )
                    });
                    (warning, "overwrite", Some(expand(definition, step_ms)))
                }
                (MacroEdit::Delete { .. }, None) => continue,
                (MacroEdit::Delete { .. }, Some(old)) => {
                    let warning = format!(
                        "Macro slot {} of slot {} holds the macro '{}'.",
                        m.get(),
                        slot.get(),
                        old.name
                    );
                    (Some(warning), "delete it", None)
                }
            };
            if let Some(declined) = warning.and_then(|w| declined(policy, &w, action)) {
                return Ok(Plan::Declined(declined));
            }
            blob = self.model.set_macro(&blob, slot, m, def.as_ref())?;
            changed = true;
        }
        if !changed {
            return Ok(Plan::Nothing(format!(
                "Macro slot {} is already empty.",
                first.macro_slot().get()
            )));
        }
        let mut pages = Vec::new();
        for edit in edits {
            let MacroEdit::Put { macro_slot, definition } = edit else { continue };
            self.check_bank(&blob, mode, slot, *macro_slot, definition)?;
            let steps = expand(definition, step_ms).steps;
            let stream = self.model.encode_macro_steps(&steps, mode)?;
            let old = match held_in(*macro_slot) {
                Some(old) => self.back_up_steps(mode, slot, *macro_slot, old.clone())?,
                None => None,
            };
            pages.push(Page { slot, macro_slot: *macro_slot, stream, steps: steps.len(), old });
        }
        bank.blob = blob;
        bank.pages.extend(pages);
        bank.macro_slots.push(slot);
        bank.dirty = true;
        Ok(Plan::Write(
            match edits {
                [MacroEdit::Put { .. }] => PUT,
                [MacroEdit::Delete { .. }] => DELETED,
                _ => EDITED,
            }
            .to_owned(),
        ))
    }

    /// Validates `def`, a macro a caller built, as [`Self::check_macro`] validates a file.
    fn check_definition(
        &self,
        def: &MacroDefinition,
        mode: Mode,
        macro_slot: MacroSlot,
    ) -> Result<MacroDefinition> {
        let device = self.model.default_profile(mode).device;
        self.check_macro(&macro_to_json(def, &device), mode, macro_slot)
    }

    /// Refuses a trigger button of `def` that starts another macro of `slot` in `bank`, or
    /// that has another output there.
    fn check_bank(
        &self,
        bank: &[u8],
        mode: Mode,
        slot: Slot,
        macro_slot: MacroSlot,
        def: &MacroDefinition,
    ) -> Result<()> {
        if def.trigger_buttons.is_empty() {
            return Ok(());
        }
        let mut others = self.model.decode_macro_metadata(bank, slot)?;
        others.retain(|m| m.macro_slot != Some(macro_slot.get()));
        let raw = RawProfilePayload {
            payload: bank.to_vec(),
            source_slot: slot.get(),
            source_profile_index: 0,
            mode_hint: mode,
        };
        let profile = self.model.map_profile(&raw)?.canonical;
        let default = self.model.default_profile(mode);
        let refusal = |b: &String| bank_refusal(b, &others, &profile, &default);
        def.trigger_buttons
            .iter()
            .find_map(refusal)
            .map_or(Ok(()), |reason| Err(Error::Validation(reason)))
    }

    /// Reads the steps of `held`, the macro a replace overwrites, before any write. A
    /// macro with no steps has nothing to back up.
    pub(super) fn back_up_steps(
        &self,
        mode: Mode,
        slot: Slot,
        macro_slot: MacroSlot,
        mut held: MacroDefinition,
    ) -> Result<Option<OldMacro>> {
        let steps = held.steps.len();
        if steps == 0 {
            return Ok(None);
        }
        let mut stream = self
            .dev
            .read_macro_stream(mode, slot, macro_slot, steps)
            .map_err(|e| Error::Usb(format!("The old macro could not be read first: {e}")))?;
        held.steps = self.model.decode_macro_steps(&stream, steps, mode)?;
        let held = fold(&held, self.model.description()?.limits.glide_step_ms);
        stream.resize(stream.len().div_ceil(MACRO_CHUNK_LEN) * MACRO_CHUNK_LEN, 0);
        Ok(Some(OldMacro { def: held, stream }))
    }
}

/// The message a declined `policy` gives, or `None` when the write may go on.
fn declined(policy: &ConfirmPolicy, warning: &str, action: &str) -> Option<String> {
    match policy {
        ConfirmPolicy::Force => None,
        ConfirmPolicy::Abort => Some(format!("{warning} Use --force to {action}.")),
        ConfirmPolicy::Ask(ask) if ask(warning) => None,
        ConfirmPolicy::Ask(_) => Some("Write aborted by user.".to_owned()),
    }
}

/// The output of `button` in `profile`. A button with no entry keeps its own press.
fn output<'a>(profile: &'a CanonicalProfile, button: &'a str) -> &'a str {
    profile.button_mappings.iter().find(|m| m.source == button).map_or(button, |m| &m.target)
}

/// The message for a trigger button of a put that the bank refuses: one that starts
/// another macro of the slot, or one with another output than in `default`, the mode's
/// default profile. A back paddle's own output reads `disabled` there, and its macro
/// plays.
fn bank_refusal(
    button: &str,
    others: &[MacroDefinition],
    profile: &CanonicalProfile,
    default: &CanonicalProfile,
) -> Option<String> {
    if let Some(other) = others.iter().find(|m| m.trigger_buttons.iter().any(|b| b == button)) {
        return Some(format!(
            "'{button}' already starts the macro '{}' in macro slot {}.",
            other.name,
            other.macro_slot.unwrap_or_default()
        ));
    }
    let target = output(profile, button);
    (target != output(default, button))
        .then(|| format!("'{button}' is mapped to '{target}', so it cannot start a macro."))
}
