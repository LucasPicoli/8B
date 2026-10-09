//! Remap and the three settings patches: read the slot, change a few fields of its
//! canonical profile, write it back. Everything else in the slot stays as read.
//!
//! Every patched field is range checked, and the slot's macros are kept.

use serde_json::Value;

use super::write::{failure_from, text, Plan, ProfileWriteOrchestrator};
use crate::description::ControllerDescription;
use crate::error::{Error, Result};
use crate::model::{ButtonMapping, CanonicalProfile, Mode, RawProfilePayload, Slot, WriteResult};
use crate::service::{ConfirmPolicy, ReadbackResult};

/// Stick fields to change. A `None` field keeps the value on the controller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StickPatch {
    /// Left stick min percent (0 to 90).
    pub left_min_pct: Option<i32>,
    /// Left stick max percent (10 to 100).
    pub left_max_pct: Option<i32>,
    /// Right stick min percent (0 to 90).
    pub right_min_pct: Option<i32>,
    /// Right stick max percent (10 to 100).
    pub right_max_pct: Option<i32>,
    /// Invert left X.
    pub invert_left_x: Option<bool>,
    /// Invert left Y.
    pub invert_left_y: Option<bool>,
    /// Invert right X.
    pub invert_right_x: Option<bool>,
    /// Invert right Y.
    pub invert_right_y: Option<bool>,
    /// Swap the two sticks.
    pub swap_sticks: Option<bool>,
    /// Swap the D-pad with the left stick. Not allowed with a left invert flag.
    pub swap_dpad_with_left_stick: Option<bool>,
}

/// Trigger fields to change. A `None` field keeps the value on the controller.
///
/// `XInput` and `DInput` use the min and max fields, Switch uses the thresholds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TriggerPatch {
    /// Left trigger min percent (0 to 100, not in Switch mode).
    pub left_min_pct: Option<i32>,
    /// Left trigger max percent (0 to 100, not in Switch mode).
    pub left_max_pct: Option<i32>,
    /// Right trigger min percent (0 to 100, not in Switch mode).
    pub right_min_pct: Option<i32>,
    /// Right trigger max percent (0 to 100, not in Switch mode).
    pub right_max_pct: Option<i32>,
    /// Left trigger threshold percent (0 to 90, Switch mode only).
    pub left_threshold_pct: Option<i32>,
    /// Right trigger threshold percent (0 to 90, Switch mode only).
    pub right_threshold_pct: Option<i32>,
    /// Swap the two triggers.
    pub swap_triggers: Option<bool>,
}

/// The last segment of a JSON pointer, the name a message shows: `left_min_pct` for
/// `/sticks/left_min_pct`.
fn name(field: &str) -> &str {
    field.rsplit('/').next().unwrap_or(field)
}

/// The set fields of `pairs` as setting changes.
fn changes<T: Into<Value> + Copy>(
    pairs: &[(&'static str, Option<T>)],
) -> Vec<(&'static str, Value)> {
    pairs.iter().filter_map(|&(field, value)| Some((field, value?.into()))).collect()
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

/// Writes each change into `profile`, then refuses a flag turned on together with one
/// it excludes.
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
    for (field, _) in changes.iter().filter(|(f, _)| on(f)) {
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
    pub fn remap_button(
        &self,
        mode: Mode,
        slot: Slot,
        source: &str,
        target: &str,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        if let Err(e) = self.model.validate_remap(mode, source, target) {
            return failure_from(mode, slot, &e);
        }
        self.patch(mode, slot, policy, "remap", "Button remapped.", |profile| {
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
    pub fn patch_settings(
        &self,
        mode: Mode,
        slot: Slot,
        changes: &[(&str, Value)],
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        self.patch_named(mode, slot, changes, policy, "Settings patched.")
    }

    /// Changes the set fields of the stick settings in an occupied slot.
    #[must_use]
    pub fn patch_sticks(
        &self,
        mode: Mode,
        slot: Slot,
        patch: &StickPatch,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        let mut set = changes(&[
            ("/sticks/left_min_pct", patch.left_min_pct),
            ("/sticks/left_max_pct", patch.left_max_pct),
            ("/sticks/right_min_pct", patch.right_min_pct),
            ("/sticks/right_max_pct", patch.right_max_pct),
        ]);
        set.extend(changes(&[
            ("/sticks/invert_left_x", patch.invert_left_x),
            ("/sticks/invert_left_y", patch.invert_left_y),
            ("/sticks/invert_right_x", patch.invert_right_x),
            ("/sticks/invert_right_y", patch.invert_right_y),
            ("/sticks/swap_sticks", patch.swap_sticks),
            ("/sticks/swap_dpad_with_left_stick", patch.swap_dpad_with_left_stick),
        ]));
        self.patch_named(mode, slot, &set, policy, "Stick settings patched.")
    }

    /// Changes the set fields of the trigger settings in an occupied slot. A field of the
    /// other trigger form is refused.
    #[must_use]
    pub fn patch_triggers(
        &self,
        mode: Mode,
        slot: Slot,
        patch: &TriggerPatch,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        let mut set = changes(&[
            ("/triggers/left_min_pct", patch.left_min_pct),
            ("/triggers/left_max_pct", patch.left_max_pct),
            ("/triggers/right_min_pct", patch.right_min_pct),
            ("/triggers/right_max_pct", patch.right_max_pct),
            ("/triggers/left_threshold_pct", patch.left_threshold_pct),
            ("/triggers/right_threshold_pct", patch.right_threshold_pct),
        ]);
        set.extend(changes(&[("/triggers/swap_triggers", patch.swap_triggers)]));
        self.patch_named(mode, slot, &set, policy, "Trigger settings patched.")
    }

    /// Sets both vibration levels in an occupied slot, in the model's range (0 to 5 on a
    /// Pro 3).
    #[must_use]
    pub fn patch_vibration(
        &self,
        mode: Mode,
        slot: Slot,
        left: u8,
        right: u8,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        let set = changes(&[
            ("/vibration/left_level", Some(left)),
            ("/vibration/right_level", Some(right)),
        ]);
        self.patch_named(mode, slot, &set, policy, "Vibration settings patched.")
    }

    /// Checks `changes` before any device access, then patches them in with `done` as
    /// the success message.
    fn patch_named(
        &self,
        mode: Mode,
        slot: Slot,
        changes: &[(&str, Value)],
        policy: &ConfirmPolicy,
        done: &str,
    ) -> WriteResult {
        let description = match self.model.description() {
            Ok(d) => d,
            Err(e) => return failure_from(mode, slot, &e),
        };
        if let Err(e) = check_changes(description, mode, changes) {
            return failure_from(mode, slot, &e);
        }
        self.patch(mode, slot, policy, "patch settings", done, |profile| {
            apply_changes(description, mode, profile, changes)
        })
    }

    /// Shared patch flow: the slot must be occupied, its profile is decoded, `edit` changes
    /// it, and it is compiled back with the slot's macros kept.
    fn patch(
        &self,
        mode: Mode,
        slot: Slot,
        policy: &ConfirmPolicy,
        what: &str,
        done: &str,
        edit: impl FnOnce(&mut CanonicalProfile) -> Result<()>,
    ) -> WriteResult {
        self.run(mode, slot, policy, done, |rb: &ReadbackResult| {
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
