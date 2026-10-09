//! Remap and the settings patch: read the slot, change a few fields of its
//! canonical profile, write it back. Everything else in the slot stays as read.
//!
//! Every patched field is range checked, and the slot's macros are kept.

use serde_json::Value;

use super::write::{failure_from, text, Plan, ProfileWriteOrchestrator};
use crate::description::ControllerDescription;
use crate::error::{Error, Result};
use crate::model::{ButtonMapping, CanonicalProfile, Mode, RawProfilePayload, Slot, WriteResult};
use crate::service::{ConfirmPolicy, ReadbackResult};

/// The last segment of a JSON pointer, the name a message shows: `left_min_pct` for
/// `/sticks/left_min_pct`.
fn name(field: &str) -> &str {
    field.rsplit('/').next().unwrap_or(field)
}

/// Checks each change against the settings `description` declares for `mode`: a number
/// in its range, or a flag. No device access.
fn check_changes(
    description: &ControllerDescription,
    mode: Mode,
    changes: &[(&str, Value)],
) -> Result<()> {
    for (field, value) in changes {
        let n = name(field);
        if let Some(number) = description.number(mode, field) {
            let (lo, hi) = (number.min, number.max);
            match value.as_i64() {
                Some(v) if (i64::from(lo)..=i64::from(hi)).contains(&v) => {}
                Some(v) => {
                    return Err(Error::Validation(format!("{n} must be {lo} to {hi}, got {v}.")));
                }
                None => return Err(Error::Validation(format!("{n} must be a whole number."))),
            }
        } else if description.flag(mode, field).is_some() {
            if !value.is_boolean() {
                return Err(Error::Validation(format!("{n} must be true or false.")));
            }
        } else {
            return Err(Error::Validation(format!("{n} does not apply in {mode} mode.")));
        }
    }
    Ok(())
}

/// The settings group of a JSON pointer: `sticks` for `/sticks/left_min_pct`.
fn group(field: &str) -> &str {
    field.trim_start_matches('/').split('/').next().unwrap_or(field)
}

/// Writes each change into `profile`, then refuses a flag that is on together with one
/// it excludes, in every group a change touches. A clash the slot already held counts
/// too, so a patch never writes one back.
fn apply_changes(
    description: &ControllerDescription,
    mode: Mode,
    profile: &mut CanonicalProfile,
    changes: &[(&str, Value)],
) -> Result<()> {
    let mut json = serde_json::to_value(&*profile)
        .map_err(|e| Error::Validation(format!("the profile cannot be edited: {e}")))?;
    for (field, value) in changes {
        let slot = json
            .pointer_mut(field)
            .ok_or_else(|| Error::Validation(format!("the profile has no {}.", name(field))))?;
        value.clone_into(slot);
    }
    let on = |field: &str| json.pointer(field).and_then(Value::as_bool) == Some(true);
    let touched = |field: &str| changes.iter().any(|(f, _)| group(f) == group(field));
    // The changed flags first, so the message names the one this patch turned on.
    let patched = changes.iter().map(|(f, _)| *f);
    let held = description.flags(mode).map(|f| f.field.as_str()).filter(|f| touched(f));
    for field in patched.chain(held).filter(|f| on(f)) {
        let clashes: Vec<&str> =
            description.excluded_by(mode, field).into_iter().filter(|f| on(f)).map(name).collect();
        let Some((last, rest)) = clashes.split_last() else { continue };
        let joined = if rest.is_empty() {
            (*last).to_owned()
        } else {
            format!("{} and {last}", rest.join(", "))
        };
        return Err(Error::Validation(format!(
            "{} cannot be on together with {joined}.",
            name(field)
        )));
    }
    *profile = serde_json::from_value(json)
        .map_err(|e| Error::Validation(format!("the edited profile is not valid: {e}")))?;
    Ok(())
}

impl ProfileWriteOrchestrator<'_> {
    /// Points `source` at `target` in an occupied slot. `target` may be `disabled`,
    /// `screenshot` in Switch mode, or a back-paddle output in `DInput` mode.
    #[must_use]
    pub fn remap_button(&self, mode: Mode, slot: Slot, source: &str, target: &str) -> WriteResult {
        if let Err(e) = self.model.validate_remap(mode, source, target) {
            return failure_from(mode, slot, &e);
        }
        self.patch(mode, slot, "remap", "Button remapped.", |profile| {
            match profile.button_mappings.iter_mut().find(|m| m.source == source) {
                Some(mapping) => target.clone_into(&mut mapping.target),
                None => profile
                    .button_mappings
                    .push(ButtonMapping { source: source.to_owned(), target: target.to_owned() }),
            }
            Ok(())
        })
    }

    /// Sets the settings at the JSON pointers of `changes` in an occupied slot, such as
    /// `("/vibration/left_level", 3)`. Each pointer must be a setting the model declares
    /// for `mode`, and each value must fit it.
    #[must_use]
    pub fn patch_settings(&self, mode: Mode, slot: Slot, changes: &[(&str, Value)]) -> WriteResult {
        let description = match self.model.description() {
            Ok(d) => d,
            Err(e) => return failure_from(mode, slot, &e),
        };
        if let Err(e) = check_changes(description, mode, changes) {
            return failure_from(mode, slot, &e);
        }
        self.patch(mode, slot, "patch settings", "Settings patched.", |profile| {
            apply_changes(description, mode, profile, changes)
        })
    }

    /// Shared patch flow: the slot must be occupied, its profile is decoded, `edit` changes
    /// it, and it is compiled back with the slot's macros kept.
    ///
    /// A patch edits the profile the slot holds, so it never asks to overwrite it.
    fn patch(
        &self,
        mode: Mode,
        slot: Slot,
        what: &str,
        done: &str,
        edit: impl FnOnce(&mut CanonicalProfile) -> Result<()>,
    ) -> WriteResult {
        self.run(mode, slot, &ConfirmPolicy::Force, done, |rb: &ReadbackResult| {
            if !rb.slot_active {
                return Err(Error::Validation(format!(
                    "Cannot {what} on an empty slot. Upload a profile first."
                )));
            }
            let raw = RawProfilePayload {
                payload: rb.backup_blob.clone(),
                source_slot: slot.get(),
                source_profile_index: 0,
                mode_hint: mode,
            };
            let mut profile = self
                .model
                .map_profile(&raw)
                .map_err(|e| {
                    Error::write(format!("Failed to decode the existing profile: {}", text(&e)))
                })?
                .canonical;
            edit(&mut profile)?;
            let blob = self
                .model
                .compile_profile_keep_macros(&profile, slot, &rb.backup_blob)
                .map_err(|e| Error::write(format!("Compilation failed: {}", text(&e))))?;
            Ok(Plan::Write(blob))
        })
    }
}
