//! The `upload-all` verb: every profile file of a folder, in one write.
//!
//! The folder is an `export` folder. Each `profile-<mode>-slot-<N>-index-<N>.json` goes
//! to the slot its name gives, behind one flip of the controller, the way the app's
//! Write all does. Any other file in the folder is left alone. All files are read and
//! checked before anything is sent, so one bad file writes nothing.
//!
//! The payload has one `results` entry per slot, in the order of the write, each shaped
//! like the payload of `upload`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use controller_core::description::ControllerDescription;
use controller_core::device::Model;
use controller_core::devices;
use controller_core::error::Error;
use controller_core::model::{Mode, Slot, WriteResult};
use controller_core::orchestrator::{MacroEdit, ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::validation::macro_stick_warnings;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo;

use crate::commands::{emit_json, error_category_label};
use crate::macros::{read_json, with_fields, with_warnings};
use crate::upload::macro_edits;
use crate::write::{confirm_policy, write_model};

/// What `upload-all` writes.
#[derive(Debug)]
pub struct UploadAll<'a> {
    /// The folder of profile files.
    pub dir: &'a Path,
    /// Replace each slot's macros with its profile's `macros`, as `upload --with-macros`
    /// does. Without it every slot keeps its own.
    pub with_macros: bool,
    /// Also empty every slot of the controller that has no file.
    pub clear_missing: bool,
}

/// Runs the `upload-all` command against the attached controller.
///
/// # Returns
/// Process exit code.
pub fn run_upload_all(req: &UploadAll<'_>, force: bool) -> i32 {
    let dev = devices::open(None);
    let (payload, code) = upload_all(dev.as_ref(), req, &confirm_policy(force));
    emit_json(&payload);
    code
}

/// A job, and what its result adds to the payload.
struct Planned {
    job: WriteJob,
    extra: Value,
}

/// Writes the folder of `req` to `dev`.
fn upload_all(dev: &dyn DeviceIo, req: &UploadAll<'_>, policy: &ConfirmPolicy) -> (Value, i32) {
    let model = match write_model(dev) {
        Ok(model) => model,
        Err(e) => return refused(&e),
    };
    let planned = match plan(model, req) {
        Ok(planned) => planned,
        Err(e) => return refused(&e),
    };
    let (jobs, extras): (Vec<WriteJob>, Vec<Value>) =
        planned.into_iter().map(|p| (p.job, p.extra)).unzip();
    let results =
        ProfileWriteOrchestrator::new(dev, model, Path::new(".")).write_slots(&jobs, policy);
    let payloads: Vec<(Value, i32)> =
        results.iter().zip(extras).map(|(r, extra)| with_fields(r, extra)).collect();
    let code = payloads.iter().map(|(_, code)| *code).find(|code| *code != 0).unwrap_or(0);
    let success = results.iter().all(|r: &WriteResult| r.success);
    let results: Vec<Value> = payloads.into_iter().map(|(payload, _)| payload).collect();
    (json!({ "success": success, "exit_code": code, "results": results }), code)
}

/// The payload and exit code of a run that failed before anything was sent.
fn refused(e: &Error) -> (Value, i32) {
    eprintln!("{e}");
    let category = e.category();
    let code = category.exit_code();
    let payload = json!({
        "success": false,
        "exit_code": code,
        "error_category": error_category_label(category),
        "message": e.to_string(),
    });
    (payload, code)
}

/// The jobs for every profile file of the folder, then one clear for each slot with no
/// file when asked, in the order the controller lists its modes and slots.
///
/// # Errors
/// [`Error::Validation`] for a folder that cannot be read or holds no profile file, and
/// for the first file that cannot be read, is not a valid profile, or has macros that
/// cannot be written.
fn plan(model: &'static dyn Model, req: &UploadAll<'_>) -> Result<Vec<Planned>, Error> {
    let description = model.description()?;
    let files = profile_files(req.dir, description)?;
    let mut planned = Vec::new();
    for (mode, slot, file) in &files {
        let named = |e: Error| {
            Error::Validation(format!(
                "Slot {} in {mode} mode, {}: {e}",
                slot.get(),
                file.display()
            ))
        };
        let profile = read_json(file).map_err(named)?;
        planned.push(job(model, req, (*mode, *slot), file, profile).map_err(named)?);
    }
    if req.clear_missing {
        for m in &description.modes {
            for n in 1..=description.slot_count {
                let slot = Slot::new(n)?;
                if !files.iter().any(|(mode, s, _)| (*mode, *s) == (m.id, slot)) {
                    let job = WriteJob { mode: m.id, slot, op: WriteOp::Clear };
                    planned.push(Planned { job, extra: json!({}) });
                }
            }
        }
    }
    if planned.is_empty() {
        return Err(Error::Validation(format!(
            "No profile-<mode>-slot-<N>-index-<N>.json file in {}.",
            req.dir.display()
        )));
    }
    Ok(planned)
}

/// The job for `profile`, the file in `slot` of `mode`. On a model with profile names, a
/// profile named "" is an empty slot of an older export, and clears the slot.
fn job(
    model: &dyn Model,
    req: &UploadAll<'_>,
    (mode, slot): (Mode, Slot),
    file: &Path,
    mut profile: Value,
) -> Result<Planned, Error> {
    // A blank name clears the slot, on a model with profile names. A model without them
    // exports every profile with a blank name, and that file must upload back.
    let renames = model.description()?.profile_rename;
    if renames && profile.get("name").and_then(Value::as_str) == Some("") {
        return Ok(Planned { job: WriteJob { mode, slot, op: WriteOp::Clear }, extra: json!({}) });
    }
    let mut extra = json!({});
    let mut macro_edits_of_job = Vec::new();
    if req.with_macros {
        let folder = file.parent().unwrap_or_else(|| Path::new("."));
        macro_edits_of_job = macro_edits(model, &profile, mode, folder)?;
        let puts: Vec<_> = macro_edits_of_job
            .iter()
            .filter_map(|e| match e {
                MacroEdit::Put { definition, .. } => Some(definition.clone()),
                MacroEdit::Delete { .. } => None,
            })
            .collect();
        let warnings = macro_stick_warnings(model.description()?, mode, &profile, &puts);
        extra = with_warnings(json!({ "macro_count": puts.len() }), &warnings);
    }
    // The job's profile carries no macros: they travel as the edits.
    if let Some(map) = profile.as_object_mut() {
        map.insert("macro_refs".to_owned(), json!([]));
        map.remove("macros");
    }
    let op = WriteOp::Upload {
        profile: Some(profile),
        drop_macros: Vec::new(),
        macro_edits: macro_edits_of_job,
    };
    Ok(Planned { job: WriteJob { mode, slot, op }, extra })
}

/// Each `profile-<mode>-slot-<N>-index-<N>.json` of `dir`, with its mode and slot, in
/// file name order.
///
/// # Errors
/// [`Error::Validation`] for a folder that cannot be read, or a file name whose mode or
/// slot the controller does not have.
fn profile_files(
    dir: &Path,
    description: &ControllerDescription,
) -> Result<Vec<(Mode, Slot, PathBuf)>, Error> {
    let read = std::fs::read_dir(dir)
        .map_err(|e| Error::Validation(format!("Cannot read folder {}: {e}", dir.display())))?;
    let mut names: Vec<PathBuf> = read.filter_map(|entry| Some(entry.ok()?.path())).collect();
    names.sort();
    let mut files = Vec::new();
    for path in names {
        let Some(rest) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("profile-"))
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        let Some((mode, tail)) = rest.split_once("-slot-") else { continue };
        let Some((slot, index)) = tail.split_once("-index-") else { continue };
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let mode = description.modes.iter().find(|m| m.id.as_str() == mode).ok_or_else(|| {
            Error::Validation(format!(
                "{} is for a mode this controller does not have.",
                path.display()
            ))
        })?;
        let slot = slot
            .parse::<u8>()
            .map_err(|e| Error::Validation(format!("{}: {e}", path.display())))
            .and_then(|n| Slot::new(n)?.check(description.slot_count))?;
        files.push((mode.id, slot, path));
    }
    Ok(files)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::device::ProtocolCodec as _;
    use controller_core::devices::pro3::{Pro3, DINPUT, XINPUT};
    use controller_core::model::{CanonicalProfile, MacroDefinition, ProfileReadResult};
    use controller_core::transport::mock::{MockCall, MockDevice, MockOp};

    use super::*;
    use crate::macros::tests::{def, pad, pages, wrote_nothing};

    /// The JSON of a profile for `mode` that embeds `macros`, when there are some.
    fn profile(mode: Mode, name: &str, macros: Option<Vec<MacroDefinition>>) -> Value {
        let profile = CanonicalProfile {
            id: format!("{mode}-new"),
            name: name.to_owned(),
            macros,
            ..Pro3.default_profile(mode)
        };
        serde_json::to_value(profile).unwrap()
    }

    /// A folder holding each of `files` under its name.
    fn folder(files: &[(&str, Value)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, json) in files {
            std::fs::write(dir.path().join(name), json.to_string()).unwrap();
        }
        dir
    }

    fn run(
        dev: &MockDevice,
        dir: &Path,
        (with_macros, clear_missing): (bool, bool),
        policy: &ConfirmPolicy,
    ) -> (Value, i32) {
        upload_all(dev, &UploadAll { dir, with_macros, clear_missing }, policy)
    }

    fn count(dev: &MockDevice, op: MockOp) -> usize {
        dev.calls().iter().filter(|c| MockCall::op(c) == op).count()
    }

    #[test]
    fn two_modes_are_written_behind_one_flip() {
        let dir = folder(&[
            ("profile-xinput-slot-2-index-1.json", profile(XINPUT, "Racing", None)),
            ("profile-dinput-slot-3-index-2.json", profile(DINPUT, "Shooter", None)),
        ]);
        let dev = pad(0);
        let (payload, code) = run(&dev, dir.path(), (false, false), &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["results"].as_array().map(Vec::len), Some(2), "{payload}");
        assert_eq!((count(&dev, MockOp::BeginWrite), count(&dev, MockOp::EndWrite)), (1, 1));
        assert_eq!(count(&dev, MockOp::WriteFullProfile), 2);
    }

    #[test]
    fn with_macros_each_slot_gets_its_own_and_reports_the_count() {
        let dir = folder(&[
            (
                "profile-xinput-slot-2-index-1.json",
                profile(XINPUT, "Racing", Some(vec![def("Dash", XINPUT, "l3", 1)])),
            ),
            ("profile-xinput-slot-3-index-2.json", profile(XINPUT, "Plain", Some(Vec::new()))),
        ]);
        let dev = pad(0);
        let (payload, code) = run(&dev, dir.path(), (true, false), &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["results"][0]["macro_count"], 1, "{payload}");
        assert_eq!(payload["results"][1]["macro_count"], 0, "{payload}");
        assert_eq!(pages(&dev), [1]);
        assert_eq!(count(&dev, MockOp::BeginWrite), 1);
    }

    #[test]
    fn without_the_flag_embedded_macros_are_not_written() {
        let dir = folder(&[(
            "profile-xinput-slot-2-index-1.json",
            profile(XINPUT, "Racing", Some(vec![def("Dash", XINPUT, "l3", 1)])),
        )]);
        let dev = pad(0);
        let (payload, code) = run(&dev, dir.path(), (false, false), &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert!(payload["results"][0].get("macro_count").is_none(), "{payload}");
        assert_eq!(pages(&dev), [0_u8; 0]);
    }

    #[test]
    fn clear_missing_empties_every_slot_without_a_file() {
        let dir =
            folder(&[("profile-xinput-slot-2-index-1.json", profile(XINPUT, "Racing", None))]);
        let dev = pad(0);
        let (payload, code) = run(&dev, dir.path(), (false, true), &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        // The Pro 3 has 3 slots in each of 3 modes.
        let results = payload["results"].as_array().unwrap();
        assert_eq!(results.len(), 9, "{payload}");
        let first = results.iter().find(|r| r["mode"] == "xinput" && r["slot"] == 1).unwrap();
        assert_eq!(first["message"], "Slot deactivated.", "{payload}");
        assert_eq!(count(&dev, MockOp::BeginWrite), 1);
    }

    #[test]
    fn a_profile_named_empty_clears_its_slot_and_other_files_are_left_alone() {
        let dir = folder(&[
            ("profile-xinput-slot-1-index-0.json", profile(XINPUT, "", None)),
            ("xinput-slot1-macro0-Dash.json", json!({ "not": "a profile" })),
            ("notes.txt", json!("ignored")),
        ]);
        let dev = pad(0);
        let (payload, code) = run(&dev, dir.path(), (false, false), &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["results"].as_array().map(Vec::len), Some(1), "{payload}");
        assert_eq!(payload["results"][0]["message"], "Slot deactivated.", "{payload}");
    }

    #[test]
    fn a_blank_name_on_a_model_without_profile_names_is_uploaded_not_cleared() {
        let ultimate = devices::model_named("Ultimate").unwrap();
        let image = |name| std::fs::read(format!("../../fixtures/ultimate/{name}.bin")).unwrap();
        let raw_blobs = vec![image("xinput-read"), image("switch-saved")];
        let dev = MockDevice::new()
            .with_model(ultimate)
            .with_profiles(ProfileReadResult { raw_blobs, ..Default::default() })
            .keeping_writes();
        // Every export of this model has a blank name: it stores none that 8B writes.
        let blank = CanonicalProfile { name: String::new(), ..ultimate.default_profile(XINPUT) };
        let dir =
            folder(&[("profile-xinput-slot-2-index-1.json", serde_json::to_value(blank).unwrap())]);
        let (payload, code) = run(&dev, dir.path(), (false, false), &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["results"][0]["message"], "Profile uploaded successfully.", "{payload}");
    }

    #[test]
    fn one_bad_file_writes_nothing_and_is_exit_4() {
        let dir =
            folder(&[("profile-xinput-slot-2-index-1.json", profile(XINPUT, "Racing", None))]);
        std::fs::write(dir.path().join("profile-xinput-slot-3-index-2.json"), "{ nope").unwrap();
        let dev = pad(0);
        let (payload, code) = run(&dev, dir.path(), (false, false), &ConfirmPolicy::Force);
        assert_eq!(code, 4, "{payload}");
        let message = payload["message"].as_str().unwrap();
        assert!(message.contains("Slot 3 in xinput mode"), "{message}");
        assert!(wrote_nothing(&dev));
    }

    #[test]
    fn a_macro_that_cannot_be_written_stops_the_whole_run() {
        let dir = folder(&[
            ("profile-xinput-slot-2-index-1.json", profile(XINPUT, "Racing", None)),
            (
                "profile-xinput-slot-3-index-2.json",
                profile(XINPUT, "Hop", Some(vec![def("Combo", DINPUT, "l3", 0)])),
            ),
        ]);
        let dev = pad(0);
        let (payload, code) = run(&dev, dir.path(), (true, false), &ConfirmPolicy::Force);
        assert_eq!(code, 4, "{payload}");
        assert!(wrote_nothing(&dev));
    }

    #[test]
    fn an_unforced_run_that_would_overwrite_a_slot_writes_nothing_and_is_exit_2() {
        let dir = folder(&[
            ("profile-xinput-slot-1-index-0.json", profile(XINPUT, "Racing", None)),
            ("profile-xinput-slot-2-index-1.json", profile(XINPUT, "Drift", None)),
        ]);
        let dev = pad(0);
        let (payload, code) = run(&dev, dir.path(), (false, false), &ConfirmPolicy::Abort);
        assert_eq!((code, &payload["success"]), (2, &json!(false)), "{payload}");
        assert!(wrote_nothing(&dev));
    }

    #[test]
    fn a_folder_with_no_profile_file_or_a_foreign_mode_is_exit_4() {
        let empty = folder(&[("notes.txt", json!("x"))]);
        let foreign = folder(&[("profile-nope-slot-1-index-0.json", profile(XINPUT, "A", None))]);
        let late = folder(&[("profile-xinput-slot-9-index-0.json", profile(XINPUT, "A", None))]);
        for (dir, says) in [(&empty, "No profile"), (&foreign, "mode"), (&late, "out of range")] {
            let dev = pad(0);
            let (payload, code) = run(&dev, dir.path(), (false, false), &ConfirmPolicy::Force);
            assert_eq!(code, 4, "{says}: {payload}");
            assert!(payload["message"].as_str().unwrap().contains(says), "{says}: {payload}");
            assert!(wrote_nothing(&dev), "{says}");
        }
    }
}
