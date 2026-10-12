//! Writing several slots in one run.
//!
//! A write puts a whole mode's bank on the controller, so one slot at a time sends the
//! same 2348 bytes again for each slot, and flips the controller and reads it again each
//! time. [`ProfileWriteOrchestrator::write_slots`] reads once, folds every slot into its
//! mode's bank, then sends each changed bank once: flip once, then slot select, write and
//! apply per bank, then flip back once.
//!
//! A job may also change the slot's macros. Its bank carries the new descriptors, and
//! each put's step page follows the bank (see [`super::pages`]). One job is one
//! rollback unit with its bank.

use serde_json::Value;

use super::pages::Page;
use super::write::{
    check_upload, failure_from, with_ignored, Plan, ProfileWriteOrchestrator, UPLOADED,
};
use crate::error::{Error, ErrorCategory, Result};
use crate::model::{CanonicalProfile, MacroDefinition, MacroSlot, Mode, Slot, WriteResult};
use crate::service::{bank_of, confirm_slot, ConfirmPolicy, ReadbackResult};

/// What a write does to its slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    /// Put this profile in the slot. The slot keeps the macros it has, except for the
    /// changes in `macro_edits`, and the buttons in `drop_macros` no longer start any of
    /// them.
    Upload {
        /// The profile, as the profile schema holds it, with no `macro_refs`. `None`
        /// keeps the profile the slot holds, and the slot must hold one.
        profile: Option<Value>,
        /// The buttons to take off the slot's macros, each one that got another output.
        drop_macros: Vec<String>,
        /// The changes to the slot's macros, at most one per macro slot. A put needs a
        /// profile in the slot, held already or carried by this job.
        macro_edits: Vec<MacroEdit>,
    },
    /// Empty the slot. Its macros stay stored.
    Clear,
}

/// One change to one macro slot of a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroEdit {
    /// Create or replace the macro in `macro_slot`.
    Put {
        /// The macro slot.
        macro_slot: MacroSlot,
        /// The macro, for the job's mode.
        definition: MacroDefinition,
    },
    /// Remove the macro in `macro_slot`. Only its descriptor is zeroed.
    Delete {
        /// The macro slot.
        macro_slot: MacroSlot,
    },
}

impl MacroEdit {
    /// The macro slot the edit changes.
    #[must_use]
    pub const fn macro_slot(&self) -> MacroSlot {
        match self {
            Self::Put { macro_slot, .. } | Self::Delete { macro_slot } => *macro_slot,
        }
    }
}

/// One write to one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteJob {
    /// The mode of the slot.
    pub mode: Mode,
    /// The slot.
    pub slot: Slot,
    /// What to do to it.
    pub op: WriteOp,
}

/// One mode's bank while a batch builds it.
pub(super) struct Bank {
    pub(super) mode: Mode,
    /// The bank as read, kept for the rollback.
    pub(super) backup: Vec<u8>,
    /// The bank with the batch's slots in it.
    pub(super) blob: Vec<u8>,
    /// The slot of the job that opened the bank. It names the bank in a failure.
    pub(super) first: Slot,
    /// A target slot held a profile, so a failed write is rolled back.
    pub(super) was_active: bool,
    /// Some job changes the bank. A bank without changes is not written.
    pub(super) dirty: bool,
    /// The step pages of the bank's puts, written after the bank.
    pub(super) pages: Vec<Page>,
    /// The slots whose macros changed, which the read-back checks.
    pub(super) macro_slots: Vec<Slot>,
    /// How the write of the bank went. `None` until it was tried.
    pub(super) result: Option<WriteResult>,
}

/// The message and profile id of a job that succeeds.
pub(super) struct Outcome {
    message: String,
    profile_id: String,
}

/// Message of a job with nothing to write.
const NOTHING: &str = "Nothing to write.";

/// Messages of a batch that failed before any write, or stopped after one failed.
const NOT_WRITTEN: &str = "Not written: an earlier slot failed.";

impl ProfileWriteOrchestrator<'_> {
    /// Writes every job in one run and returns one result per job, in the order of `jobs`.
    ///
    /// The controller is read once. Each mode's bank is built from that read with all its
    /// jobs applied, so the other slots and the stored macros stay as they were. Then each
    /// changed bank goes out once, in the order its first job appears, between one flip
    /// of the controller and its return. A bank whose write fails is rolled back, the
    /// banks after it are not written, and the banks before it stay written.
    ///
    /// Nothing is sent when a job is invalid, a slot's overwrite is declined by `policy`,
    /// or the read fails: every result then holds that failure. A job that fails inside a
    /// bank fails every job of that bank, because the bank is the unit that is written.
    /// Callers that retry go on from the first failed result.
    #[must_use]
    pub fn write_slots(&self, jobs: &[WriteJob], policy: &ConfirmPolicy) -> Vec<WriteResult> {
        let Some(head) = jobs.first() else { return Vec::new() };
        let (mut banks, outcomes) = match self.plan(jobs, policy) {
            Ok(plan) => plan,
            Err(failure) => return everywhere(jobs, &failure),
        };
        if banks.iter().any(|b| b.dirty) {
            let back_to = match self.dev.begin_write() {
                Ok(back_to) => back_to,
                Err(e) => return everywhere(jobs, &failure_from(head.mode, head.slot, &e)),
            };
            let mut stopped = false;
            for bank in banks.iter_mut().filter(|b| b.dirty) {
                let result = if stopped {
                    WriteResult::failure(bank.mode, bank.first, ErrorCategory::None, NOT_WRITTEN)
                } else {
                    self.write_bank(bank)
                };
                stopped = !result.success;
                bank.result = Some(result);
            }
            self.read_back(&mut banks);
            let mut results = collect(jobs, &banks, outcomes);
            if let Some(back_to) = back_to {
                if let Err(e) = self.dev.end_write(back_to) {
                    for r in &mut results {
                        r.message = format!(
                            "{} The controller did not return to {back_to} mode ({e}); \
                             unplug it and plug it back in.",
                            r.message
                        );
                    }
                }
            }
            return results;
        }
        collect(jobs, &banks, outcomes)
    }

    /// Reads the controller once and builds every bank. No write happens here.
    fn plan(
        &self,
        jobs: &[WriteJob],
        policy: &ConfirmPolicy,
    ) -> std::result::Result<(Vec<Bank>, Vec<Outcome>), WriteResult> {
        // With several jobs a failure names the slot it is about.
        let named = jobs.len() > 1;
        let fail = |job: &WriteJob, e: &Error| {
            let mut f = failure_from(job.mode, job.slot, e);
            if named {
                f.message = format!("Slot {} in {} mode: {}", job.slot.get(), job.mode, f.message);
            }
            f
        };
        let Some(head) = jobs.first() else { return Ok((Vec::new(), Vec::new())) };
        for job in jobs {
            self.check_slot(job.slot)
                .and_then(|()| self.check_mode(job.mode))
                .map_err(|e| fail(job, &e))?;
        }
        let read = self.dev.read_all_profiles().map_err(|e| fail(head, &e))?;
        self.check_model().map_err(|e| fail(head, &e))?;
        let mut banks: Vec<Bank> = Vec::new();
        let mut outcomes = Vec::new();
        for (i, job) in jobs.iter().enumerate() {
            if jobs.iter().take(i).any(|j| (j.mode, j.slot) == (job.mode, job.slot)) {
                let e = Error::Validation("the batch writes this slot twice".to_owned());
                return Err(fail(job, &e));
            }
            let parsed = match &job.op {
                WriteOp::Upload { profile: Some(profile), .. } => {
                    Some(check_upload(self.model, profile, job.mode).map_err(|e| fail(job, &e))?)
                }
                _ => None,
            };
            if !banks.iter().any(|b| b.mode == job.mode) {
                let backup =
                    bank_of(self.model, &read, job.mode).map_err(|e| fail(job, &e))?.clone();
                banks.push(Bank {
                    mode: job.mode,
                    blob: backup.clone(),
                    backup,
                    first: job.slot,
                    was_active: false,
                    dirty: false,
                    pages: Vec::new(),
                    macro_slots: Vec::new(),
                    result: None,
                });
            }
            let bank = banks
                .iter_mut()
                .find(|b| b.mode == job.mode)
                .ok_or_else(|| fail(job, &Error::Usb("the bank went missing".to_owned())))?;
            // A job that keeps the slot's profile overwrites none, so only its macros ask.
            let keeps_profile = matches!(job.op, WriteOp::Upload { profile: None, .. });
            let slot_policy = if keeps_profile { &ConfirmPolicy::Force } else { policy };
            let rb = confirm_slot(self.model, &bank.backup, job.mode, job.slot, slot_policy)
                .map_err(|e| fail(job, &e))?;
            if !rb.proceed {
                return Err(WriteResult::failure(
                    job.mode,
                    job.slot,
                    ErrorCategory::None,
                    rb.message,
                ));
            }
            bank.was_active |= rb.slot_active;
            outcomes.push(match &job.op {
                WriteOp::Upload { .. } => {
                    match self
                        .plan_upload(bank, job, parsed, &rb, policy)
                        .map_err(|e| fail(job, &e))?
                    {
                        Plan::Write(outcome) => outcome,
                        Plan::Nothing(message) | Plan::Declined(message) => {
                            let none = ErrorCategory::None;
                            return Err(WriteResult::failure(job.mode, job.slot, none, message));
                        }
                    }
                }
                WriteOp::Clear if !rb.slot_active => Outcome {
                    message: format!(
                        "Slot {} in {} mode is already empty.",
                        job.slot.get(),
                        job.mode
                    ),
                    profile_id: String::new(),
                },
                WriteOp::Clear => {
                    bank.blob = self
                        .model
                        .deactivate_profile(&bank.blob, job.slot)
                        .map_err(|e| fail(job, &e))?;
                    bank.dirty = true;
                    Outcome { message: "Slot deactivated.".to_owned(), profile_id: String::new() }
                }
            });
        }
        Ok((banks, outcomes))
    }

    /// Builds an upload job into `bank`: its profile or the slot's with `drop_macros`,
    /// then its macro edits. `parsed` is its profile, checked. Returns the job's outcome,
    /// or `Declined` with the message of a macro the policy keeps.
    fn plan_upload(
        &self,
        bank: &mut Bank,
        job: &WriteJob,
        parsed: Option<CanonicalProfile>,
        rb: &ReadbackResult,
        policy: &ConfirmPolicy,
    ) -> Result<Plan<Outcome>> {
        let WriteOp::Upload { drop_macros, .. } = &job.op else {
            return Ok(Plan::Write(Outcome {
                message: NOTHING.to_owned(),
                profile_id: String::new(),
            }));
        };
        let carries_profile = parsed.is_some();
        let mut outcome = Outcome { message: NOTHING.to_owned(), profile_id: String::new() };
        if let Some(parsed) = parsed {
            bank.blob =
                self.build_upload(&parsed, job.slot, drop_macros, &bank.blob, rb.slot_active)?;
            bank.dirty = true;
            outcome.message = with_ignored(UPLOADED.to_owned(), parsed.macro_refs.len());
            outcome.profile_id = parsed.id;
        } else if rb.slot_active && !drop_macros.is_empty() {
            bank.blob = self.model.drop_macros(&bank.blob, job.slot, drop_macros)?;
            bank.dirty = true;
        }
        match self.plan_macros(bank, job, rb, carries_profile, policy)? {
            Plan::Declined(message) => return Ok(Plan::Declined(message)),
            Plan::Write(message) | Plan::Nothing(message)
                if !carries_profile && !message.is_empty() =>
            {
                outcome.message = message;
            }
            _ => {}
        }
        Ok(Plan::Write(outcome))
    }
}

/// `failure` for every job, as the slot of each.
fn everywhere(jobs: &[WriteJob], failure: &WriteResult) -> Vec<WriteResult> {
    jobs.iter()
        .map(|j| WriteResult { mode: j.mode, slot: j.slot.get(), ..failure.clone() })
        .collect()
}

/// One result per job: the failure of its bank, or a success with its own message.
fn collect(jobs: &[WriteJob], banks: &[Bank], outcomes: Vec<Outcome>) -> Vec<WriteResult> {
    jobs.iter()
        .zip(outcomes)
        .map(|(job, out)| {
            let failed = banks
                .iter()
                .find(|b| b.mode == job.mode)
                .and_then(|b| b.result.as_ref())
                .filter(|r| !r.success);
            let mut result = match failed {
                Some(r) => WriteResult { mode: job.mode, slot: job.slot.get(), ..r.clone() },
                None => WriteResult::success(job.mode, job.slot, out.message),
            };
            result.profile_id = out.profile_id;
            result
        })
        .collect()
}
