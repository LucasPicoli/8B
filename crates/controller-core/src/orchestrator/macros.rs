//! Put and delete one macro: the bank with the new or zeroed descriptor, then for a
//! put the macro's step page.
//!
//! A put sends slot select, the bank, `APPLY`, then erases the page, writes the step
//! chunks and sends `APPLY` again. A delete only zeroes the descriptor: the page stays.
//! Each stage that fails has its own rollback:
//!
//! 1. The bank write fails: the bank is restored, as for a profile.
//! 2. The erase or a step chunk fails: the old bank is restored and, on a replace, the
//!    old step stream read before the write goes back on the page. If that fails, the
//!    old bank goes out with this descriptor zeroed, so no descriptor points at a broken
//!    page, and the old macro is saved as a macro file in the backup directory.
//! 3. The last `APPLY` fails: reported, no rollback, as for a profile.
//!
//! After the last `APPLY` the bank and the page are read back and compared.

use serde_json::Value;

use super::write::{failure_from, text, Plan, ProfileWriteOrchestrator};
use crate::error::{Error, ErrorCategory, Result};
use crate::model::macros::{macro_from_json, macro_to_json};
use crate::model::{
    CanonicalProfile, MacroDefinition, MacroSlot, Mode, RawProfilePayload, Slot, WriteResult,
};
use crate::protocol::wire_write::MACRO_CHUNK_LEN;
use crate::service::validation::validate_macro;
use crate::service::{
    attempt_rollback, bank_of, save_macro_backup, ConfirmPolicy, FailedWrite, ReadbackResult,
};

/// Success message of a put.
const PUT: &str = "Macro written.";
/// Success message of a delete.
const DELETED: &str = "Macro deleted.";

/// What a macro write sends, as its build step planned it.
pub(super) struct MacroPlan {
    /// The bank with the macro's descriptor written or zeroed.
    bank: Vec<u8>,
    /// The step page of a put. A delete leaves the page alone.
    page: Option<Page>,
    /// The macro the slot held before a replace, with its steps. The rollback puts it
    /// back.
    old: Option<OldMacro>,
}

/// A step stream to write, padded to whole chunks.
struct Page {
    stream: Vec<u8>,
    /// How many steps it holds, the count a read-back asks for.
    steps: usize,
}

/// The macro a replace overwrites, read before the write.
pub(super) struct OldMacro {
    /// Its descriptor fields and decoded steps, saved as a macro file if it is lost.
    def: MacroDefinition,
    /// Its step stream as the pad held it, padded to whole chunks.
    stream: Vec<u8>,
}

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
        let def = match self.check_macro(macro_json, mode, macro_slot) {
            Ok(def) => def,
            Err(e) => return failure_from(mode, slot, &e),
        };
        self.run_job(
            mode,
            slot,
            &ConfirmPolicy::Force,
            |rb| self.build_put(rb, mode, slot, macro_slot, &def, policy),
            |rb, plan| self.write_macro(mode, slot, macro_slot, &rb.backup_blob, plan, PUT),
        )
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
        if let Err(e) = self.check_macro_slot(macro_slot) {
            return failure_from(mode, slot, &e);
        }
        self.run_job(
            mode,
            slot,
            &ConfirmPolicy::Force,
            |rb| self.build_delete(rb, slot, macro_slot, policy),
            |rb, plan| self.write_macro(mode, slot, macro_slot, &rb.backup_blob, plan, DELETED),
        )
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
        let description = self.model.description()?;
        let max = description.limits.macro_steps.max;
        if usize::try_from(max).is_ok_and(|max| def.steps.len() > max) {
            return Err(Error::Validation(format!(
                "The macro has {} steps; the {} holds at most {max}.",
                def.steps.len(),
                description.short_name
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

    /// The build step of a put: refuses an empty profile slot, asks `policy` before a
    /// replace, runs the bank checks, and reads the old macro's steps as the backup.
    pub(super) fn build_put(
        &self,
        rb: &ReadbackResult,
        mode: Mode,
        slot: Slot,
        macro_slot: MacroSlot,
        def: &MacroDefinition,
        policy: &ConfirmPolicy,
    ) -> Result<Plan<MacroPlan>> {
        let held = self.held_macro(rb, slot, macro_slot)?;
        if let Some(held) = &held {
            let warning = format!(
                "Macro slot {} of slot {} in {mode} mode already holds the macro '{}'.",
                macro_slot.get(),
                slot.get(),
                held.name
            );
            if let Some(declined) = declined(policy, &warning, "overwrite") {
                return Ok(Plan::Declined(declined));
            }
        }
        self.check_bank(&rb.backup_blob, mode, slot, macro_slot, def)?;
        let bank = self.model.set_macro(&rb.backup_blob, slot, macro_slot, Some(def))?;
        let stream = self.model.encode_macro_steps(&def.steps, mode)?;
        let old = match held {
            Some(held) => self.back_up_steps(mode, slot, macro_slot, held)?,
            None => None,
        };
        Ok(Plan::Write(MacroPlan {
            bank,
            page: Some(Page { stream, steps: def.steps.len() }),
            old,
        }))
    }

    /// The build step of a delete: refuses an empty profile slot, writes nothing for an
    /// empty macro slot, and asks `policy` before the macro goes.
    pub(super) fn build_delete(
        &self,
        rb: &ReadbackResult,
        slot: Slot,
        macro_slot: MacroSlot,
        policy: &ConfirmPolicy,
    ) -> Result<Plan<MacroPlan>> {
        let n = macro_slot.get();
        let Some(held) = self.held_macro(rb, slot, macro_slot)? else {
            return Ok(Plan::Nothing(format!("Macro slot {n} is already empty.")));
        };
        let warning =
            format!("Macro slot {n} of slot {} holds the macro '{}'.", slot.get(), held.name);
        if let Some(declined) = declined(policy, &warning, "delete it") {
            return Ok(Plan::Declined(declined));
        }
        let bank = self.model.set_macro(&rb.backup_blob, slot, macro_slot, None)?;
        Ok(Plan::Write(MacroPlan { bank, page: None, old: None }))
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

    /// The macro in `macro_slot` of the bank as read, with no steps read yet. Refuses a
    /// profile slot that holds no profile.
    fn held_macro(
        &self,
        rb: &ReadbackResult,
        slot: Slot,
        macro_slot: MacroSlot,
    ) -> Result<Option<MacroDefinition>> {
        if !rb.slot_active {
            return Err(Error::Validation(format!(
                "Write a profile to slot {} first.",
                slot.get()
            )));
        }
        let held = self.model.decode_macro_metadata(&rb.backup_blob, slot)?;
        Ok(held.into_iter().find(|m| m.macro_slot == Some(macro_slot.get())))
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
        stream.resize(stream.len().div_ceil(MACRO_CHUNK_LEN) * MACRO_CHUNK_LEN, 0);
        Ok(Some(OldMacro { def: held, stream }))
    }

    /// The write stage of a put or a delete, after the flip: the bank, then the page of
    /// a put, then the read-back check. `backup` is the bank as read.
    pub(super) fn write_macro(
        &self,
        mode: Mode,
        slot: Slot,
        macro_slot: MacroSlot,
        backup: &[u8],
        plan: MacroPlan,
        done: &str,
    ) -> WriteResult {
        let result = self.write(mode, slot, backup, true, &plan.bank, done);
        if !result.success {
            return result;
        }
        if let Some(page) = &plan.page {
            if let Err(e) = self.dev.write_macro_stream(mode, slot, macro_slot, &page.stream) {
                return self.roll_back_page(mode, slot, macro_slot, backup, plan.old, &e);
            }
            if let Err(e) = self.dev.send_apply(mode) {
                return WriteResult::failure(
                    mode,
                    slot,
                    ErrorCategory::WriteFailure,
                    format!("Write succeeded but APPLY failed: {e}"),
                );
            }
        }
        match self.read_back(mode, slot, macro_slot, &plan) {
            Ok(None) => result,
            Ok(Some(what)) => WriteResult::failure(
                mode,
                slot,
                ErrorCategory::WriteFailure,
                format!("Read-back check failed: the controller holds another {what}."),
            ),
            Err(e) => failure_from(mode, slot, &e),
        }
    }

    /// Stage 2 of the rollback: the erase or a step chunk failed after the new bank went
    /// out. Restores the old bank, then the old steps of a replace.
    fn roll_back_page(
        &self,
        mode: Mode,
        slot: Slot,
        macro_slot: MacroSlot,
        backup: &[u8],
        old: Option<OldMacro>,
        err: &Error,
    ) -> WriteResult {
        let failed = FailedWrite::from_error(err);
        let mut result =
            attempt_rollback(self.dev, mode, slot, backup, true, failed, self.backup_dir);
        let Some(old) = old else { return result };
        if result.rollback_succeeded {
            let rewritten = self
                .dev
                .write_macro_stream(mode, slot, macro_slot, &old.stream)
                .and_then(|()| self.dev.send_apply(mode));
            if rewritten.is_ok() {
                return result;
            }
            result.rollback_succeeded = false;
            let emptied = self
                .model
                .set_macro(backup, slot, macro_slot, None)
                .and_then(|bank| self.dev.write_full_profile(mode, &bank))
                .and_then(|()| self.dev.send_apply(mode));
            result.message = format!(
                "{} The old macro's steps could not be written back, {}",
                failed.prefix(),
                if emptied.is_ok() {
                    format!("so macro slot {} is now empty.", macro_slot.get())
                } else {
                    "and its macro slot could not be emptied. Unplug the controller.".to_owned()
                }
            );
        }
        let device = self.model.default_profile(mode).device;
        let saved = save_macro_backup(
            mode,
            slot,
            macro_slot,
            &macro_to_json(&old.def, &device),
            self.backup_dir,
        );
        result.message = match saved {
            Ok(path) => format!("{} The old macro is saved to {}.", result.message, path.display()),
            Err(e) => format!("{} The old macro could not be saved: {}", result.message, text(&e)),
        };
        result
    }

    /// Reads the bank and the page back and compares them with what was sent. Returns
    /// the part that differs, or `None`. The bank is compared by its decoded macros of
    /// `slot`, so bytes the pad fills in itself do not count.
    fn read_back(
        &self,
        mode: Mode,
        slot: Slot,
        macro_slot: MacroSlot,
        plan: &MacroPlan,
    ) -> Result<Option<&'static str>> {
        let read = self.dev.read_all_profiles()?;
        let held = self.model.decode_macro_metadata(bank_of(self.model, &read, mode)?, slot)?;
        if held != self.model.decode_macro_metadata(&plan.bank, slot)? {
            return Ok(Some("macro table"));
        }
        if let Some(page) = &plan.page {
            let back = self.dev.read_macro_stream(mode, slot, macro_slot, page.steps)?;
            if back.is_empty() || !page.stream.starts_with(&back) {
                return Ok(Some("step page"));
            }
        }
        Ok(None)
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
