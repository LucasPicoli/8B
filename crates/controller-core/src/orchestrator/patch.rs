//! Remap and the three settings patches: read the slot, change a few fields of its
//! canonical profile, write it back. Everything else in the slot stays as read.
//!
//! Ports `remapButton`, `patchSticks`, `patchTriggers` and `patchVibration`. Unlike the
//! C++ code, every patched field is range checked here, and the slot's macros are kept.

use super::write::{failure_from, text, Plan, ProfileWriteOrchestrator};
use crate::devices::pro3::tables;
use crate::error::{Error, Result};
use crate::model::{
    ButtonMapping, CanonicalProfile, Mode, RawProfilePayload, Slot, Triggers, WriteResult,
};
use crate::service::validation::dpad_swap_clash;
use crate::service::{ConfirmPolicy, ReadbackResult};

/// Upper bound of a trigger min or max percent.
const TRIGGER_PCT_MAX: i32 = 100;

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
    /// Points `source` at `target` in an occupied slot. `target` may be `disabled`, or
    /// `screenshot` in Switch mode.
    #[must_use]
    pub fn remap_button(
        &self,
        mode: Mode,
        slot: Slot,
        source: &str,
        target: &str,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        if let Err(e) = self.codec.validate_remap(mode, source, target) {
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
        let ranges = check_ranges(&[
            (
                "left_min_pct",
                patch.left_min_pct,
                tables::STICK_MIN_PCT_LO,
                tables::STICK_MIN_PCT_HI,
            ),
            (
                "left_max_pct",
                patch.left_max_pct,
                tables::STICK_MAX_PCT_LO,
                tables::STICK_MAX_PCT_HI,
            ),
            (
                "right_min_pct",
                patch.right_min_pct,
                tables::STICK_MIN_PCT_LO,
                tables::STICK_MIN_PCT_HI,
            ),
            (
                "right_max_pct",
                patch.right_max_pct,
                tables::STICK_MAX_PCT_LO,
                tables::STICK_MAX_PCT_HI,
            ),
        ]);
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
        let checked = if mode == Mode::Switch {
            if analog.iter().any(Option::is_some) {
                Err(Error::Validation(
                    "left_min_pct, left_max_pct, right_min_pct and right_max_pct do not apply in switch mode. Use left_threshold_pct and right_threshold_pct."
                        .to_owned(),
                ))
            } else {
                check_ranges(&[
                    (
                        "left_threshold_pct",
                        patch.left_threshold_pct,
                        tables::SWITCH_THRESHOLD_PCT_LO,
                        tables::SWITCH_THRESHOLD_PCT_HI,
                    ),
                    (
                        "right_threshold_pct",
                        patch.right_threshold_pct,
                        tables::SWITCH_THRESHOLD_PCT_LO,
                        tables::SWITCH_THRESHOLD_PCT_HI,
                    ),
                ])
            }
        } else if thresholds.iter().any(Option::is_some) {
            Err(Error::Validation(
                "left_threshold_pct and right_threshold_pct only apply in switch mode. Use left_min_pct, left_max_pct, right_min_pct and right_max_pct."
                    .to_owned(),
            ))
        } else {
            check_ranges(&[
                ("left_min_pct", patch.left_min_pct, 0, TRIGGER_PCT_MAX),
                ("left_max_pct", patch.left_max_pct, 0, TRIGGER_PCT_MAX),
                ("right_min_pct", patch.right_min_pct, 0, TRIGGER_PCT_MAX),
                ("right_max_pct", patch.right_max_pct, 0, TRIGGER_PCT_MAX),
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

    /// Sets both vibration levels (0 to 5) in an occupied slot.
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
        let levels = check_ranges(&[
            ("left vibration level", Some(left), 0, tables::VIBRATION_LEVEL_MAX),
            ("right vibration level", Some(right), 0, tables::VIBRATION_LEVEL_MAX),
        ]);
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
                .codec
                .map_profile(&raw)
                .map_err(|e| {
                    Error::write(format!("Failed to decode the existing profile: {}", text(&e)))
                })?
                .canonical;
            edit(&mut profile)?;
            let blob = self
                .codec
                .compile_profile_keep_macros(&profile, slot, &rb.backup_blob)
                .map_err(|e| Error::write(format!("Compilation failed: {}", text(&e))))?;
            Ok(Plan::Write(blob))
        })
    }
}
