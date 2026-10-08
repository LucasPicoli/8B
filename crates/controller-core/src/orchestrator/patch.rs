//! Remap and the three settings patches: read the slot, change a few fields of its
//! canonical profile, write it back. Everything else in the slot stays as read.
//!
//! Ports `remapButton`, `patchSticks`, `patchTriggers` and `patchVibration`. Unlike the
//! C++ code, every patched field is range checked here, and the slot's macros are kept.

use super::write::{failure_from, text, Plan, ProfileWriteOrchestrator};
use crate::description::{Limits, TriggerKind};
use crate::error::{Error, Result};
use crate::model::{
    ButtonMapping, CanonicalProfile, Mode, RawProfilePayload, Slot, Triggers, WriteResult,
};
use crate::service::validation::dpad_swap_clash;
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

/// Overwrites `target` when `value` is set.
fn set<T>(target: &mut T, value: Option<T>) {
    if let Some(v) = value {
        *target = v;
    }
}

/// Checks each set `(name, value, low, high)`.
fn check_ranges(fields: &[(&str, Option<i32>, i32, i32)]) -> Result<()> {
    for &(name, value, lo, hi) in fields {
        if let Some(v) = value.filter(|v| !(lo..=hi).contains(v)) {
            return Err(Error::Validation(format!("{name} must be {lo} to {hi}, got {v}.")));
        }
    }
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

    /// Changes the set fields of the stick settings in an occupied slot.
    #[must_use]
    pub fn patch_sticks(
        &self,
        mode: Mode,
        slot: Slot,
        patch: &StickPatch,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        let ranges = self.limits().and_then(|l| {
            let (min, max) = (l.stick_min_pct, l.stick_max_pct);
            check_ranges(&[
                ("left_min_pct", patch.left_min_pct, min.min, min.max),
                ("left_max_pct", patch.left_max_pct, max.min, max.max),
                ("right_min_pct", patch.right_min_pct, min.min, min.max),
                ("right_max_pct", patch.right_max_pct, max.min, max.max),
            ])
        });
        if let Err(e) = ranges {
            return failure_from(mode, slot, &e);
        }
        self.patch(mode, slot, policy, "patch sticks", "Stick settings patched.", |profile| {
            let s = &mut profile.sticks;
            set(&mut s.left_min_pct, patch.left_min_pct);
            set(&mut s.left_max_pct, patch.left_max_pct);
            set(&mut s.right_min_pct, patch.right_min_pct);
            set(&mut s.right_max_pct, patch.right_max_pct);
            set(&mut s.invert_left_x, patch.invert_left_x);
            set(&mut s.invert_left_y, patch.invert_left_y);
            set(&mut s.invert_right_x, patch.invert_right_x);
            set(&mut s.invert_right_y, patch.invert_right_y);
            set(&mut s.swap_sticks, patch.swap_sticks);
            set(&mut s.swap_dpad_with_left_stick, patch.swap_dpad_with_left_stick);
            if let Some(reason) = dpad_swap_clash(s) {
                return Err(Error::Validation(reason));
            }
            Ok(())
        })
    }

    /// Changes the set fields of the trigger settings in an occupied slot.
    #[must_use]
    pub fn patch_triggers(
        &self,
        mode: Mode,
        slot: Slot,
        patch: &TriggerPatch,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        let analog =
            [patch.left_min_pct, patch.left_max_pct, patch.right_min_pct, patch.right_max_pct];
        let thresholds = [patch.left_threshold_pct, patch.right_threshold_pct];
        let (limits, kind) = match self.limits().and_then(|l| Ok((l, self.trigger_kind(mode)?))) {
            Ok(found) => found,
            Err(e) => return failure_from(mode, slot, &e),
        };
        let checked = if kind == TriggerKind::Threshold {
            if analog.iter().any(Option::is_some) {
                Err(Error::Validation(
                    "left_min_pct, left_max_pct, right_min_pct and right_max_pct do not apply in switch mode. Use left_threshold_pct and right_threshold_pct."
                        .to_owned(),
                ))
            } else {
                let range = limits.trigger_threshold_pct;
                check_ranges(&[
                    ("left_threshold_pct", patch.left_threshold_pct, range.min, range.max),
                    ("right_threshold_pct", patch.right_threshold_pct, range.min, range.max),
                ])
            }
        } else if thresholds.iter().any(Option::is_some) {
            Err(Error::Validation(
                "left_threshold_pct and right_threshold_pct only apply in switch mode. Use left_min_pct, left_max_pct, right_min_pct and right_max_pct."
                    .to_owned(),
            ))
        } else {
            let range = limits.trigger_pct;
            check_ranges(&[
                ("left_min_pct", patch.left_min_pct, range.min, range.max),
                ("left_max_pct", patch.left_max_pct, range.min, range.max),
                ("right_min_pct", patch.right_min_pct, range.min, range.max),
                ("right_max_pct", patch.right_max_pct, range.min, range.max),
            ])
        };
        if let Err(e) = checked {
            return failure_from(mode, slot, &e);
        }
        self.patch(mode, slot, policy, "patch triggers", "Trigger settings patched.", |profile| {
            match &mut profile.triggers {
                Triggers::Switch(t) => {
                    set(&mut t.left_threshold_pct, patch.left_threshold_pct);
                    set(&mut t.right_threshold_pct, patch.right_threshold_pct);
                    set(&mut t.swap_triggers, patch.swap_triggers);
                }
                Triggers::Analog(t) => {
                    set(&mut t.left_min_pct, patch.left_min_pct);
                    set(&mut t.left_max_pct, patch.left_max_pct);
                    set(&mut t.right_min_pct, patch.right_min_pct);
                    set(&mut t.right_max_pct, patch.right_max_pct);
                    set(&mut t.swap_triggers, patch.swap_triggers);
                }
            }
            Ok(())
        })
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
        let (left, right) = (i32::from(left), i32::from(right));
        let levels = self.limits().and_then(|l| {
            let range = l.vibration_level;
            check_ranges(&[
                ("left vibration level", Some(left), range.min, range.max),
                ("right vibration level", Some(right), range.min, range.max),
            ])
        });
        if let Err(e) = levels {
            return failure_from(mode, slot, &e);
        }
        self.patch(
            mode,
            slot,
            policy,
            "patch vibration",
            "Vibration settings patched.",
            |profile| {
                profile.vibration.left_level = left;
                profile.vibration.right_level = right;
                Ok(())
            },
        )
    }

    /// The value ranges of the model's description.
    fn limits(&self) -> Result<Limits> {
        Ok(self.model.description()?.limits)
    }

    /// How the model tunes the triggers in `mode`.
    fn trigger_kind(&self, mode: Mode) -> Result<TriggerKind> {
        let description = self.model.description()?;
        description
            .mode(mode)
            .map(|m| m.trigger_kind)
            .ok_or_else(|| Error::Validation(format!("This controller has no {mode} mode.")))
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
