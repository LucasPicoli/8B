//! The `upload` verb, and with `--with-macros` the macros its profile holds.
//!
//! With the flag, the slot's macros become the profile's `macros`, or for an older file
//! the macro files in its `macro_refs`, in the same write job as the profile: each macro
//! is put in its macro slot, and every other macro slot is emptied.
//!
//! A `warnings` list names each macro the slot's dead zone stops from moving a stick:
//! with the flag the macros it writes, without it the macros the slot keeps.

use std::path::{Component, Path};

use serde_json::{json, Value};

use controller_core::device::{ControllerSpec, Model};
use controller_core::devices;
use controller_core::error::Error;
use controller_core::model::macros::{macro_from_json, parse_macro_file_name};
use controller_core::model::{
    CanonicalProfile, MacroDefinition, MacroRef, MacroSlot, Mode, Slot, WriteResult,
};
use controller_core::orchestrator::{MacroEdit, WriteJob, WriteOp};
use controller_core::service::read::read_macros;
use controller_core::service::validation::{
    low_sticks, macro_stick_warnings, validate_macro, validate_profile,
};
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo;

use crate::commands::emit_json;
use crate::macros::{read_json, with_fields, with_warnings};
use crate::write::{confirm_policy, orchestrate};

/// What `upload` writes, and where.
#[derive(Debug)]
pub struct Upload<'a> {
    /// The canonical profile JSON.
    pub file: &'a Path,
    /// The target mode.
    pub mode: Mode,
    /// The target slot.
    pub slot: Slot,
    /// Replace the slot's macros with the profile's `macros`, or the files its
    /// `macro_refs` name.
    pub with_macros: bool,
}

/// Runs the `upload` command against the attached controller.
///
/// A missing, unreadable or non-JSON file is a validation failure, and nothing is sent.
///
/// # Returns
/// Process exit code.
pub fn run_upload(req: &Upload<'_>, force: bool) -> i32 {
    let dev = devices::open(None);
    let (payload, code) = upload(dev.as_ref(), req, &confirm_policy(force));
    emit_json(&payload);
    code
}

/// Writes the profile file of `req` to `dev`, with its macros when asked.
fn upload(dev: &dyn DeviceIo, req: &Upload<'_>, policy: &ConfirmPolicy) -> (Value, i32) {
    let (mode, slot) = (req.mode, req.slot);
    let fail = |e: &Error| WriteResult::failure(mode, slot, e.category(), e.to_string());
    let profile = match read_json(req.file) {
        Ok(profile) => profile,
        Err(e) => {
            eprintln!("{e}");
            return with_fields(&fail(&e), json!({}));
        }
    };
    if !req.with_macros {
        let r = orchestrate(dev, mode, slot, |o, _| o.upload_profile(&profile, mode, slot, policy));
        let warnings =
            if r.success { kept_warnings(dev, mode, slot, &profile) } else { Vec::new() };
        return with_fields(&r, with_warnings(json!({}), &warnings));
    }
    let folder = req.file.parent().unwrap_or_else(|| Path::new("."));
    let mut puts = Vec::new();
    let r = orchestrate(dev, mode, slot, |o, model| {
        let edits = match macro_edits(model, &profile, mode, folder) {
            Ok(edits) => edits,
            Err(e) => return fail(&e),
        };
        puts = edits
            .iter()
            .filter_map(|e| match e {
                MacroEdit::Put { definition, .. } => Some(definition.clone()),
                MacroEdit::Delete { .. } => None,
            })
            .collect();
        // The job's profile carries no macros: they travel as the edits.
        let mut profile = profile.clone();
        if let Some(map) = profile.as_object_mut() {
            map.insert("macro_refs".to_owned(), json!([]));
            map.remove("macros");
        }
        let op =
            WriteOp::Upload { profile: Some(profile), drop_macros: Vec::new(), macro_edits: edits };
        o.write_slots(&[WriteJob { mode, slot, op }], policy)
            .pop()
            .unwrap_or_else(|| fail(&Error::Usb("the write gave no result".to_owned())))
    });
    let warnings = match (r.success, dev.model().and_then(ControllerSpec::description)) {
        (true, Ok(description)) => macro_stick_warnings(description, mode, &profile, &puts),
        _ => Vec::new(),
    };
    with_fields(&r, with_warnings(json!({ "macro_count": puts.len() }), &warnings))
}

/// The macro stick warnings for the macros `slot` of `mode` on `dev` keeps under
/// `profile`. Reads them only when a stick of `profile` is under the mode's floor. A
/// failed read gives none: the write is done, and this only warns.
fn kept_warnings(dev: &dyn DeviceIo, mode: Mode, slot: Slot, profile: &Value) -> Vec<String> {
    let found = || -> Result<Vec<String>, Error> {
        let description = dev.model()?.description()?;
        if low_sticks(description, mode, profile).is_empty() {
            return Ok(Vec::new());
        }
        let macros = read_macros(dev, mode, slot)?.macros;
        Ok(macro_stick_warnings(description, mode, profile, &macros))
    };
    found().unwrap_or_default()
}

/// The edits that make the slot's macros the ones `profile` embeds in `macros`, or else
/// the files in its `macro_refs`, read from `folder`: a put per macro, in its macro slot,
/// and a delete for every other macro slot of `model`.
///
/// # Errors
/// [`Error::Validation`] for a profile with `macros` that is not valid, or naming the
/// ref for a path that leaves `folder`, a file name with no macro number, a macro number
/// two refs share, a file that is missing or invalid, is for another mode than `mode`,
/// or names other buttons than its ref.
fn macro_edits(
    model: &dyn Model,
    profile: &Value,
    mode: Mode,
    folder: &Path,
) -> Result<Vec<MacroEdit>, Error> {
    let mut taken: Vec<u8> = Vec::new();
    let mut edits = Vec::new();
    if profile.get("macros").is_some() {
        for definition in embedded_macros(model, profile)? {
            let n = definition.macro_slot.unwrap_or_default();
            taken.push(n);
            edits.push(MacroEdit::Put { macro_slot: MacroSlot::new(n)?, definition });
        }
    }
    let refs: Vec<MacroRef> = match profile.get("macro_refs") {
        Some(refs) => serde_json::from_value(refs.clone())
            .map_err(|e| Error::Validation(format!("The profile's macro_refs: {e}")))?,
        None => Vec::new(),
    };
    for r in &refs {
        let n = macro_number(&r.path)?;
        if taken.contains(&n) {
            return Err(Error::Validation(format!(
                "Two macro refs name macro slot {n}; '{}' is the second.",
                r.path
            )));
        }
        taken.push(n);
        let mut definition = read_macro(model, &folder.join(&r.path), r, mode)?;
        definition.macro_slot = Some(n);
        edits.push(MacroEdit::Put { macro_slot: MacroSlot::new(n)?, definition });
    }
    for m in 0..model.description()?.macro_slot_count {
        if !taken.contains(&m) {
            edits.push(MacroEdit::Delete { macro_slot: MacroSlot::new(m)? });
        }
    }
    Ok(edits)
}

/// The macros `profile` embeds, each with its macro slot, once the whole profile is
/// valid for `model`. Validation refuses non-empty `macro_refs` beside them.
///
/// # Errors
/// [`Error::Validation`] listing every reason the profile is not valid.
fn embedded_macros(model: &dyn Model, profile: &Value) -> Result<Vec<MacroDefinition>, Error> {
    let validation = validate_profile(model, profile)?;
    if !validation.valid {
        let details: Vec<String> =
            validation.errors.iter().map(|e| format!("{}: {}", e.path, e.reason)).collect();
        return Err(Error::Validation(format!(
            "Profile validation failed: {}",
            details.join("; ")
        )));
    }
    let parsed: CanonicalProfile = serde_json::from_value(profile.clone())
        .map_err(|e| Error::Validation(format!("The profile's macros: {e}")))?;
    Ok(parsed.macros.unwrap_or_default())
}

/// The macro slot in the file name of the ref `path`.
///
/// # Errors
/// [`Error::Validation`] for a path that leaves the profile's folder (`..` or a root)
/// or a file name with no macro number.
fn macro_number(path: &str) -> Result<u8, Error> {
    let p = Path::new(path);
    if p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(Error::Validation(format!(
            "The macro ref '{path}' leaves the profile's folder."
        )));
    }
    p.file_name()
        .and_then(|name| name.to_str())
        .and_then(parse_macro_file_name)
        .map(|(n, _)| n)
        .ok_or_else(|| {
            Error::Validation(format!(
                "The macro ref '{path}' has no macro number. Name the file \
                 <mode>-slot<N>-macro<M>-<name>.json."
            ))
        })
}

/// Reads the macro file at `file` that `r` names, and checks it against `model`, `mode`
/// and the buttons of `r`.
///
/// # Errors
/// [`Error::Validation`] for a file that is missing or invalid, for another mode, or
/// with other buttons than `r`.
fn read_macro(
    model: &dyn Model,
    file: &Path,
    r: &MacroRef,
    mode: Mode,
) -> Result<MacroDefinition, Error> {
    let shown = file.display();
    let json = read_json(file)?;
    let errors = validate_macro(model, &json)?;
    if !errors.is_empty() {
        let details: Vec<String> =
            errors.iter().map(|e| format!("{}: {}", e.path, e.reason)).collect();
        return Err(Error::Validation(format!(
            "The macro file {shown} is not valid: {}",
            details.join("; ")
        )));
    }
    let def = macro_from_json(&json)?;
    if def.mode != mode {
        return Err(Error::Validation(format!(
            "The macro file {shown} is for {} mode, not {mode}.",
            def.mode
        )));
    }
    let sorted = |b: &[String]| {
        let mut b = b.to_vec();
        b.sort();
        b
    };
    if sorted(&r.trigger_buttons) != sorted(&def.trigger_buttons) {
        return Err(Error::Validation(format!(
            "The macro ref '{}' names the buttons {:?}, but its file names {:?}.",
            r.path, r.trigger_buttons, def.trigger_buttons
        )));
    }
    Ok(def)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::device::ProtocolCodec as _;
    use controller_core::devices::pro3::{Pro3, DINPUT, XINPUT};
    use controller_core::model::macros::{macro_file_name, macro_to_json};
    use controller_core::model::MacroStep;
    use controller_core::transport::mock::{MockCall, MockDevice, MockOp};

    use super::*;
    use crate::macros::tests::{def, pad, pad_with, pages, slot1, stick, wrote_nothing};

    /// A folder with an `XInput` profile whose `macro_refs` are `refs`, and each macro
    /// of `files` beside it under its own file name.
    fn folder(refs: Value, files: &[MacroDefinition]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut profile = serde_json::to_value(Pro3.default_profile(XINPUT)).unwrap();
        profile["id"] = "xinput-new".into();
        profile["name"] = "New".into();
        profile["macro_refs"] = refs;
        std::fs::write(dir.path().join("profile.json"), profile.to_string()).unwrap();
        for d in files {
            let json = macro_to_json(d, &Pro3.default_profile(d.mode).device);
            let name = macro_file_name(XINPUT, 1, d);
            std::fs::write(dir.path().join(name), json.to_string()).unwrap();
        }
        dir
    }

    /// A folder with an `XInput` profile that embeds `macros`, and `refs` beside them.
    fn embedded(macros: Vec<MacroDefinition>, refs: Value) -> tempfile::TempDir {
        let dir = folder(refs, &[]);
        let profile = CanonicalProfile {
            id: "xinput-new".to_owned(),
            name: "New".to_owned(),
            macros: Some(macros),
            ..Pro3.default_profile(XINPUT)
        };
        let mut json = serde_json::to_value(profile).unwrap();
        let path = dir.path().join("profile.json");
        let text = std::fs::read_to_string(&path).unwrap();
        json["macro_refs"] = serde_json::from_str::<Value>(&text).unwrap()["macro_refs"].clone();
        std::fs::write(path, json.to_string()).unwrap();
        dir
    }

    /// The ref of `d` as `export` writes it.
    fn reference(d: &MacroDefinition) -> Value {
        json!({ "trigger_buttons": d.trigger_buttons, "path": macro_file_name(XINPUT, 1, d) })
    }

    fn run(
        dev: &MockDevice,
        dir: &Path,
        with_macros: bool,
        policy: &ConfirmPolicy,
    ) -> (Value, i32) {
        let file = dir.join("profile.json");
        let req = Upload { file: &file, mode: XINPUT, slot: slot1(), with_macros };
        upload(dev, &req, policy)
    }

    /// The macro slot and name of each macro the pad holds in `XInput` slot 1.
    fn held(dev: &MockDevice) -> Vec<(Option<u8>, String)> {
        let bank = &dev.read_all_profiles().unwrap().raw_blobs[0];
        let macros = Pro3.decode_macro_metadata(bank, slot1()).unwrap();
        macros.into_iter().map(|m| (m.macro_slot, m.name)).collect()
    }

    #[test]
    fn the_refs_replace_every_macro_of_the_slot_in_one_job() {
        use MockOp::{Apply, BeginWrite, EndWrite, SlotSelect, WriteFullProfile, WriteMacroStream};
        let dev = pad(2);
        let (a, b) = (def("Dash", XINPUT, "l3", 0), def("Hop", XINPUT, "r4", 2));
        let dir = folder(json!([reference(&a), reference(&b)]), &[a, b]);
        let (payload, code) = run(&dev, dir.path(), true, &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["macro_count"], 2);
        assert_eq!(held(&dev), [(Some(0), "Dash".to_owned()), (Some(2), "Hop".to_owned())]);
        // The bank with the new descriptors, then each page, under one flip.
        let ops: Vec<MockOp> = dev.calls().iter().map(MockCall::op).collect();
        assert_eq!(
            ops,
            [
                BeginWrite,
                SlotSelect,
                WriteFullProfile,
                Apply,
                WriteMacroStream,
                WriteMacroStream,
                Apply,
                EndWrite
            ]
        );
        assert_eq!(pages(&dev), [0, 2]);
    }

    #[test]
    fn embedded_macros_replace_every_macro_of_the_slot_in_one_job() {
        let dev = pad(2);
        let dir =
            embedded(vec![def("Dash", XINPUT, "l3", 1), def("Hop", XINPUT, "r4", 3)], json!([]));
        let (payload, code) = run(&dev, dir.path(), true, &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["macro_count"], 2);
        assert!(!payload["message"].as_str().unwrap().contains("Ignored"), "{payload}");
        assert_eq!(held(&dev), [(Some(1), "Dash".to_owned()), (Some(3), "Hop".to_owned())]);
        assert_eq!(pages(&dev), [1, 3]);
    }

    #[test]
    fn an_empty_macros_list_clears_the_slots_macros() {
        let dev = pad(2);
        let dir = embedded(Vec::new(), json!([]));
        let (payload, code) = run(&dev, dir.path(), true, &ConfirmPolicy::Force);
        assert_eq!((code, &payload["macro_count"]), (0, &json!(0)), "{payload}");
        assert_eq!(held(&dev), []);
    }

    #[test]
    fn without_the_flag_embedded_macros_are_ignored() {
        let dev = pad(2);
        let dir = embedded(vec![def("Dash", XINPUT, "l3", 0)], json!([]));
        let (payload, code) = run(&dev, dir.path(), false, &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert!(payload["message"].as_str().unwrap().contains("Ignored 1 macro"), "{payload}");
        assert_eq!(held(&dev).len(), 2);
    }

    #[test]
    fn an_invalid_embedded_macro_is_exit_4_with_nothing_written() {
        let a = def("Dash", XINPUT, "l3", 0);
        let cases = [
            (vec![a.clone()], json!([reference(&a)]), "/macro_refs"),
            (vec![a.clone(), def("Hop", XINPUT, "r4", 0)], json!([]), "/macros/1/macro_slot"),
            (vec![def("Combo", DINPUT, "l3", 1)], json!([]), "/macros/0/mode"),
        ];
        for (macros, refs, says) in cases {
            let dev = pad(2);
            let dir = embedded(macros, refs);
            let (payload, code) = run(&dev, dir.path(), true, &ConfirmPolicy::Force);
            assert_eq!(code, 4, "{says}: {payload}");
            assert!(payload["message"].as_str().unwrap().contains(says), "{says}: {payload}");
            assert!(wrote_nothing(&dev), "{says}");
        }
    }

    #[test]
    fn no_refs_clear_the_slots_macros() {
        let dev = pad(2);
        let dir = folder(json!([]), &[]);
        let (payload, code) = run(&dev, dir.path(), true, &ConfirmPolicy::Force);
        assert_eq!((code, &payload["macro_count"]), (0, &json!(0)), "{payload}");
        assert_eq!(held(&dev), []);
        assert_eq!(pages(&dev), [0_u8; 0]);
    }

    #[test]
    fn without_the_flag_the_slot_keeps_its_macros() {
        let dev = pad(2);
        let a = def("Dash", XINPUT, "l3", 0);
        let dir = folder(json!([reference(&a)]), &[a]);
        let (payload, code) = run(&dev, dir.path(), false, &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert!(payload.get("macro_count").is_none());
        assert_eq!(held(&dev).len(), 2);
    }

    /// Sets the left stick's dead zone low end of the profile in `dir` to 0.
    fn low_left_stick(dir: &Path) {
        let file = dir.join("profile.json");
        let mut profile: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        profile["sticks"]["left_min_pct"] = 0.into();
        std::fs::write(file, profile.to_string()).unwrap();
    }

    #[test]
    fn a_stick_macro_file_under_a_low_dead_zone_warns() {
        let a = stick(def("Dash", XINPUT, "l3", 0));
        let dir = folder(json!([reference(&a)]), &[a]);
        low_left_stick(dir.path());
        let (payload, code) = run(&pad(0), dir.path(), true, &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["warnings"].as_array().map(Vec::len), Some(1), "{payload}");
    }

    #[test]
    fn a_kept_stick_macro_under_a_low_dead_zone_warns() {
        let profile = Pro3.default_profile(XINPUT);
        let dev = pad_with(&profile, &[stick(def("Dash", XINPUT, "l3", 0))]);
        let dir = folder(json!([]), &[]);
        let (payload, code) = run(&dev, dir.path(), false, &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert!(payload.get("warnings").is_none(), "{payload}");

        low_left_stick(dir.path());
        let (payload, code) = run(&dev, dir.path(), false, &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        let warnings = payload["warnings"].as_array().unwrap();
        assert!(warnings[0].as_str().unwrap().contains("'Dash' moves the left stick"), "{payload}");
    }

    #[test]
    fn an_unforced_upload_names_each_macro_the_slot_loses() {
        let dev = pad(2);
        let dir = folder(json!([]), &[]);
        let (payload, code) = run(&dev, dir.path(), true, &ConfirmPolicy::Abort);
        assert_eq!(code, 2, "{payload}");
        let message = payload["message"].as_str().unwrap();
        assert!(message.contains("'Held0' in macro slot 0"), "{message}");
        assert!(message.contains("'Held1' in macro slot 1"), "{message}");
        assert!(wrote_nothing(&dev));
    }

    #[test]
    fn each_bad_ref_is_exit_4_with_nothing_written() {
        let good = def("Dash", XINPUT, "l3", 0);
        let other_mode = def("Combo", DINPUT, "l3", 1);
        let bad_step = MacroDefinition {
            steps: vec![MacroStep {
                pressed_buttons: vec!["nope".to_owned()],
                ..MacroStep::default()
            }],
            ..def("Bad", XINPUT, "l3", 1)
        };
        let cases = [
            (
                json!([{ "trigger_buttons": ["l3"], "path": "../xinput-slot1-macro0-Dash.json" }]),
                "leaves",
            ),
            (json!([{ "trigger_buttons": ["l3"], "path": "dash.json" }]), "no macro number"),
            (json!([reference(&good), reference(&good)]), "Two macro refs"),
            (
                json!([{ "trigger_buttons": ["r1"], "path": macro_file_name(XINPUT, 1, &good) }]),
                "names the buttons",
            ),
            (json!([reference(&other_mode)]), "for dinput mode"),
            (json!([reference(&bad_step)]), "is not valid"),
            (
                json!([{ "trigger_buttons": ["l3"], "path": "xinput-slot1-macro3-Gone.json" }]),
                "Cannot read",
            ),
        ];
        for (refs, says) in cases {
            let dev = pad(2);
            let dir = folder(refs.clone(), &[good.clone(), other_mode.clone(), bad_step.clone()]);
            let (payload, code) = run(&dev, dir.path(), true, &ConfirmPolicy::Force);
            assert_eq!(code, 4, "{refs}: {payload}");
            assert!(payload["message"].as_str().unwrap().contains(says), "{refs}: {payload}");
            assert!(wrote_nothing(&dev), "{refs}");
        }
    }
}
