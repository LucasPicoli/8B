//! The write stage of a bank that carries macro edits: the bank, then each put's step
//! page, then the read-back check.
//!
//! A bank goes out as slot select, the bank, `APPLY`. Then each put erases its page and
//! writes its step chunks, and one `APPLY` follows the last page. A delete only zeroes
//! its descriptor in the bank: the page stays. Each stage that fails has its own
//! rollback:
//!
//! 1. The bank write fails: the bank is restored, as for a profile.
//! 2. An erase or a step chunk fails: the old bank is restored and the old step stream
//!    of each replace whose page was touched goes back, as read before the write. A
//!    stream that cannot go back leaves its descriptor zeroed in the old bank, so no
//!    descriptor points at a broken page, and its macro is saved as a macro file in the
//!    backup directory.
//! 3. The last `APPLY` fails: reported, no rollback, as for a profile.
//!
//! After every bank went out, the banks and pages are read back and compared.

use std::fmt::Write as _;

use super::batch::Bank;
use super::write::{failure_from, text, ProfileWriteOrchestrator};
use crate::error::{Error, ErrorCategory, Result};
use crate::model::macros::macro_to_json;
use crate::model::{MacroDefinition, MacroSlot, Slot, WriteResult};
use crate::service::{attempt_rollback, bank_of, save_macro_backup, FailedWrite};
use crate::transport::device_io::MacroStream;

/// The step page of one put, padded to whole chunks.
pub(super) struct Page {
    /// The profile slot that holds the macro.
    pub(super) slot: Slot,
    /// The macro slot, which picks the page.
    pub(super) macro_slot: MacroSlot,
    pub(super) stream: Vec<u8>,
    /// How many steps it holds, the count a read-back asks for.
    pub(super) steps: usize,
    /// The macro a replace overwrites. The rollback puts it back.
    pub(super) old: Option<OldMacro>,
}

/// The macro a replace overwrites, read before the write.
pub(super) struct OldMacro {
    /// Its descriptor fields and decoded steps, saved as a macro file if it is lost.
    pub(super) def: MacroDefinition,
    /// Its step stream as the pad held it, padded to whole chunks.
    pub(super) stream: Vec<u8>,
}

impl ProfileWriteOrchestrator<'_> {
    /// Sends `bank`: slot select, the bank, `APPLY`, then its pages and one `APPLY`.
    pub(super) fn write_bank(&self, bank: &Bank) -> WriteResult {
        let (mode, slot) = (bank.mode, bank.first);
        let result = self.write(mode, slot, &bank.backup, bank.was_active, &bank.blob, "");
        if !result.success || bank.pages.is_empty() {
            return result;
        }
        for (i, page) in bank.pages.iter().enumerate() {
            if let Err(e) =
                self.dev.write_macro_stream(mode, page.slot, page.macro_slot, &page.stream)
            {
                let touched = bank.pages.iter().take(i + 1);
                return self.roll_back_pages(
                    bank,
                    touched.filter_map(|p| p.old.as_ref().map(|o| (p, o))),
                    &e,
                );
            }
        }
        if let Err(e) = self.dev.send_apply(mode) {
            return WriteResult::failure(
                mode,
                slot,
                ErrorCategory::WriteFailure,
                format!("Write succeeded but APPLY failed: {e}"),
            );
        }
        result
    }

    /// Stage 2 of the rollback: an erase or a step chunk failed after the new bank went
    /// out. Restores the old bank, then the old steps of each replace in `olds`.
    fn roll_back_pages<'p>(
        &self,
        bank: &Bank,
        olds: impl Iterator<Item = (&'p Page, &'p OldMacro)>,
        err: &Error,
    ) -> WriteResult {
        let (mode, slot) = (bank.mode, bank.first);
        let failed = FailedWrite::from_error(err);
        let mut result =
            attempt_rollback(self.dev, mode, slot, &bank.backup, true, failed, self.backup_dir);
        let olds: Vec<_> = olds.collect();
        if olds.is_empty() {
            return result;
        }
        let mut lost = olds.clone();
        if result.rollback_succeeded {
            lost.retain(|(page, old)| {
                self.dev.write_macro_stream(mode, page.slot, page.macro_slot, &old.stream).is_err()
            });
            if lost.is_empty() && self.dev.send_apply(mode).is_err() {
                lost = olds;
            }
            if lost.is_empty() {
                return result;
            }
            result.rollback_succeeded = false;
            let emptied = lost
                .iter()
                .try_fold(bank.backup.clone(), |b, (p, _)| {
                    self.model.set_macro(&b, p.slot, p.macro_slot, None)
                })
                .and_then(|b| self.dev.write_full_profile(mode, &b))
                .and_then(|()| self.dev.send_apply(mode));
            result.message = failed.prefix();
            for (page, _) in &lost {
                let _ = write!(
                    result.message,
                    " The old steps of macro slot {} of slot {} could not be written back{}",
                    page.macro_slot.get(),
                    page.slot.get(),
                    if emptied.is_ok() { ", so it is now empty." } else { "." }
                );
            }
            if emptied.is_err() {
                result
                    .message
                    .push_str(" Those macro slots could not be emptied. Unplug the controller.");
            }
        }
        let device = self.model.default_profile(mode).device;
        for (page, old) in &lost {
            let json = macro_to_json(&old.def, &device);
            match save_macro_backup(mode, page.slot, page.macro_slot, &json, self.backup_dir) {
                Ok(path) => {
                    let shown = path.display().to_string();
                    let _ = write!(result.message, " The old macro is saved to {shown}.");
                    result.macro_backup_paths.push(shown);
                }
                Err(e) => {
                    let _ =
                        write!(result.message, " The old macro could not be saved: {}", text(&e));
                }
            }
        }
        result
    }

    /// Reads back every bank that went out with macro edits and compares its slots'
    /// macros and its pages with what was sent. A bank that differs fails. The macros
    /// are compared decoded, so bytes the pad fills in itself do not count.
    pub(super) fn read_back(&self, banks: &mut [Bank]) {
        let checked = |b: &&mut Bank| {
            b.result.as_ref().is_some_and(|r| r.success) && !b.macro_slots.is_empty()
        };
        let mut banks: Vec<&mut Bank> = banks.iter_mut().filter(checked).collect();
        if banks.is_empty() {
            return;
        }
        let differs = self.read_back_all(&banks);
        for (i, bank) in banks.iter_mut().enumerate() {
            let failure = match &differs {
                Err(e) => failure_from(bank.mode, bank.first, e),
                Ok(differs) => match differs.get(i) {
                    Some(Some(what)) => WriteResult::failure(
                        bank.mode,
                        bank.first,
                        ErrorCategory::WriteFailure,
                        format!("Read-back check failed: the controller holds another {what}."),
                    ),
                    _ => continue,
                },
            };
            bank.result = Some(failure);
        }
    }

    /// For each of `banks`, the part the controller holds otherwise, or `None`. One read
    /// of the banks and one session for every page.
    fn read_back_all(&self, banks: &[&mut Bank]) -> Result<Vec<Option<&'static str>>> {
        let read = self.dev.read_all_profiles()?;
        let streams: Vec<MacroStream> = banks
            .iter()
            .flat_map(|b| {
                b.pages.iter().map(|p| MacroStream {
                    mode: b.mode,
                    profile_slot: p.slot,
                    macro_slot: p.macro_slot,
                    step_count: p.steps,
                })
            })
            .collect();
        let back =
            if streams.is_empty() { Vec::new() } else { self.dev.read_macro_streams(&streams)? };
        let mut back = back.into_iter();
        let mut differs = Vec::new();
        for bank in banks {
            let got: Vec<Vec<u8>> = back.by_ref().take(bank.pages.len()).collect();
            let held = bank_of(self.model, &read, bank.mode)?;
            let mut what = None;
            for &slot in &bank.macro_slots {
                let sent = self.model.decode_macro_metadata(&bank.blob, slot)?;
                if self.model.decode_macro_metadata(held, slot)? != sent {
                    what = Some("macro table");
                }
            }
            let same =
                |(page, got): (&Page, &Vec<u8>)| !got.is_empty() && page.stream.starts_with(got);
            if what.is_none()
                && (got.len() != bank.pages.len() || !bank.pages.iter().zip(&got).all(same))
            {
                what = Some("step page");
            }
            differs.push(what);
        }
        Ok(differs)
    }
}
