//! Macro write verbs: write-macro and delete-macro.
//!
//! Each runs one macro edit through [`ProfileWriteOrchestrator`] and prints its
//! [`WriteResult`] as the other write verbs do, plus the macro slot. A write adds the
//! macro's name, and with `--convert` the list of what the conversion dropped.

use std::path::Path;

use serde_json::{json, Value};

use controller_core::convert_macro::{convert_macro, MacroLoss, TargetBank};
use controller_core::device::Model;
use controller_core::devices;
use controller_core::error::{Error, ErrorCategory};
use controller_core::model::macros::{macro_from_json, macro_to_json};
use controller_core::model::{MacroSlot, Mode, Slot, WriteResult};
use controller_core::service::read::macro_summaries;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo;

use crate::commands::emit_json;
use crate::write::{build_write_payload, confirm_policy, orchestrate};

/// What `write-macro` writes, and where.
#[derive(Debug)]
pub struct MacroWrite<'a> {
    /// The `macro-v1` file.
    pub file: &'a Path,
    /// The target mode.
    pub mode: Mode,
    /// The profile slot that takes the macro.
    pub slot: Slot,
    /// The macro slot, or `None` for the first free one.
    pub macro_slot: Option<MacroSlot>,
    /// Convert a macro made for another mode instead of refusing it.
    pub convert: bool,
}

/// The macro a write sends, once its file is read and its macro slot is known.
struct Planned {
    json: Value,
    macro_slot: MacroSlot,
    name: String,
    /// What `--convert` dropped; `None` without the flag.
    dropped: Option<Vec<MacroLoss>>,
}

/// Runs the `write-macro` command against the attached controller.
///
/// # Returns
/// Process exit code.
pub fn run_write_macro(req: &MacroWrite<'_>, force: bool) -> i32 {
    let dev = devices::open(None);
    let (payload, code) = write_macro(dev.as_ref(), req, &confirm_policy(force));
    emit_json(&payload);
    code
}

/// Runs the `delete-macro` command against the attached controller.
///
/// # Returns
/// Process exit code.
pub fn run_delete_macro(mode: Mode, slot: Slot, macro_slot: MacroSlot, force: bool) -> i32 {
    let dev = devices::open(None);
    let (payload, code) =
        delete_macro(dev.as_ref(), mode, slot, macro_slot, &confirm_policy(force));
    emit_json(&payload);
    code
}

/// Deletes the macro in `macro_slot` of `slot` in `mode` on `dev`. An empty macro slot
/// is a success with no write.
fn delete_macro(
    dev: &dyn DeviceIo,
    mode: Mode,
    slot: Slot,
    macro_slot: MacroSlot,
    policy: &ConfirmPolicy,
) -> (Value, i32) {
    let r = orchestrate(dev, mode, slot, |o, _| o.delete_macro(mode, slot, macro_slot, policy));
    with_fields(&r, json!({ "macro_slot": macro_slot.get() }))
}

/// Writes the macro file of `req` to `dev`.
fn write_macro(dev: &dyn DeviceIo, req: &MacroWrite<'_>, policy: &ConfirmPolicy) -> (Value, i32) {
    let (mode, slot) = (req.mode, req.slot);
    let mut planned = None;
    let r = orchestrate(dev, mode, slot, |o, model| match plan(dev, model, req) {
        Ok(p) => {
            let r = o.put_macro(&p.json, mode, slot, p.macro_slot, policy);
            planned = Some(p);
            r
        }
        Err(r) => r,
    });
    let Some(p) = planned else { return with_fields(&r, json!({})) };
    let mut fields = serde_json::Map::new();
    fields.insert("macro_slot".to_owned(), json!(p.macro_slot.get()));
    fields.insert("name".to_owned(), json!(p.name));
    if let Some(dropped) = p.dropped {
        let dropped: Vec<Value> =
            dropped.iter().map(|l| json!({ "item": l.item, "reason": l.reason })).collect();
        fields.insert("dropped".to_owned(), json!(dropped));
    }
    with_fields(&r, Value::Object(fields))
}

/// Reads the file of `req`, picks its macro slot and converts it when asked. Reads the
/// pad only to pick the first free macro slot or to convert.
fn plan(
    dev: &dyn DeviceIo,
    model: &'static dyn Model,
    req: &MacroWrite<'_>,
) -> Result<Planned, WriteResult> {
    let (mode, slot) = (req.mode, req.slot);
    let fail = |e: &Error| WriteResult::failure(mode, slot, e.category(), e.to_string());
    let json = read_json(req.file).map_err(|e| fail(&e))?;
    let def = macro_from_json(&json).map_err(|e| fail(&e))?;
    if def.mode != mode && !req.convert {
        return Err(fail(&Error::Validation(format!(
            "The macro is for {} mode, not {mode}. Use --convert to convert it.",
            def.mode
        ))));
    }
    if let (Some(macro_slot), false) = (req.macro_slot, req.convert) {
        return Ok(Planned { json, macro_slot, name: def.name, dropped: None });
    }

    let description = model.description().map_err(|e| fail(&e))?;
    let read = dev.read_all_profiles().map_err(|e| fail(&e))?;
    // An empty slot's descriptors are leftovers, not macros, and the core refuses the
    // write anyway.
    let profile = read
        .profiles
        .iter()
        .find(|p| p.mode == mode && p.source_slot == slot.get() && !p.id.is_empty());
    let held = profile
        .and_then(|_| macro_summaries(model, &read).remove(&(mode, slot.get())))
        .unwrap_or_default();
    let count = description.macro_slot_count;
    let free = (0..count).find(|&m| !held.iter().any(|h| h.macro_slot == Some(m)));
    let macro_slot = match (req.macro_slot, free) {
        (Some(m), _) => m,
        (None, Some(m)) => MacroSlot::new(m).map_err(|e| fail(&e))?,
        (None, None) => {
            let message = format!(
                "All {count} macro slots of slot {} in {mode} mode hold a macro. Name one \
                 with --macro-slot to replace it.",
                slot.get()
            );
            return Err(WriteResult::failure(mode, slot, ErrorCategory::None, message));
        }
    };
    if !req.convert {
        return Ok(Planned { json, macro_slot, name: def.name, dropped: None });
    }

    let others: Vec<_> =
        held.into_iter().filter(|h| h.macro_slot != Some(macro_slot.get())).collect();
    let default = model.default_profile(mode);
    let bank = TargetBank {
        others: &others,
        profile: profile.map_or(&default, |p| &p.canonical),
        default: &default,
    };
    let (def, dropped) = convert_macro(description, &def, mode, &bank).map_err(|e| fail(&e))?;
    let json = macro_to_json(&def, &default.device);
    Ok(Planned { json, macro_slot, name: def.name, dropped: Some(dropped) })
}

/// Reads the JSON file at `file`.
///
/// # Errors
/// [`Error::Validation`] when the file cannot be read or is not JSON, so nothing is sent.
fn read_json(file: &Path) -> Result<Value, Error> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| Error::Validation(format!("Cannot read file {}: {e}", file.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| Error::Validation(format!("File {} is not JSON: {e}", file.display())))
}

/// The write payload of `r` with the fields of `extra` added, and its exit code.
fn with_fields(r: &WriteResult, extra: Value) -> (Value, i32) {
    let (mut payload, code) = build_write_payload(r, &[]);
    if let (Some(map), Value::Object(extra)) = (payload.as_object_mut(), extra) {
        map.extend(extra);
    }
    (payload, code)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::device::{ControllerSpec as _, ProtocolCodec as _};
    use controller_core::devices::pro3::{Pro3, DINPUT, SWITCH, XINPUT};
    use controller_core::model::{
        CanonicalProfile, MacroDefinition, MacroStep, ProfileReadResult, RawProfilePayload,
    };
    use controller_core::transport::mock::{MockCall, MockDevice};

    use super::*;

    fn slot1() -> Slot {
        Slot::new(1).unwrap()
    }

    /// A 2-step macro for `mode` in `macro_slot` that `trigger` starts.
    fn def(name: &str, mode: Mode, trigger: &str, macro_slot: u8) -> MacroDefinition {
        let step = |t| MacroStep {
            duration_ms: 40,
            pressed_buttons: vec!["bottom face".to_owned()],
            trigger_left: t,
            ..MacroStep::default()
        };
        MacroDefinition {
            name: name.to_owned(),
            mode,
            trigger_buttons: vec![trigger.to_owned()],
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps: vec![step(100), step(0)],
            macro_slot: Some(macro_slot),
        }
    }

    /// A pad whose `XInput` slot 1 holds a profile with `held` macros, in macro slots 0
    /// up, started by `l1`, `r1`, `l4` and `r4`. It keeps what it is sent.
    fn pad(held: u8) -> MockDevice {
        let triggers = ["l1", "r1", "l4", "r4"];
        let macros: Vec<MacroDefinition> = (0..held)
            .map(|m| def(&format!("Held{m}"), XINPUT, triggers[usize::from(m)], m))
            .collect();
        let profile = CanonicalProfile { name: "Pad".to_owned(), ..Pro3.default_profile(XINPUT) };
        let bank = Pro3.compile_profile(&profile, slot1(), &[], &macros).unwrap();
        let empty = vec![0; Pro3.blob_size()];
        let profiles = (1..=3)
            .map(|n| {
                let raw = RawProfilePayload {
                    payload: bank.clone(),
                    source_slot: n,
                    source_profile_index: n - 1,
                    mode_hint: XINPUT,
                };
                let mut p = Pro3.map_profile(&raw).unwrap();
                if n > 1 {
                    p.id = String::new();
                }
                p
            })
            .collect();
        let raw_blobs = vec![bank, empty.clone(), empty];
        let mut dev = MockDevice::new()
            .with_model(&Pro3)
            .with_profiles(ProfileReadResult { profiles, raw_blobs })
            .keeping_writes();
        for m in &macros {
            let stream = Pro3.encode_macro_steps(&m.steps, XINPUT).unwrap();
            let at = MacroSlot::new(m.macro_slot.unwrap()).unwrap();
            dev = dev.with_macro_stream(XINPUT, slot1(), at, stream);
        }
        dev
    }

    /// Writes `def` as a file, then runs `write-macro` with it on `dev`.
    fn write(
        dev: &MockDevice,
        def: &MacroDefinition,
        macro_slot: Option<u8>,
        convert: bool,
        policy: &ConfirmPolicy,
    ) -> (Value, i32) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("macro.json");
        let json = macro_to_json(def, &Pro3.default_profile(def.mode).device);
        std::fs::write(&file, json.to_string()).unwrap();
        let req = MacroWrite {
            file: &file,
            mode: XINPUT,
            slot: slot1(),
            macro_slot: macro_slot.map(|m| MacroSlot::new(m).unwrap()),
            convert,
        };
        write_macro(dev, &req, policy)
    }

    /// The macro slots of the pages `dev` was sent.
    fn pages(dev: &MockDevice) -> Vec<u8> {
        dev.calls()
            .iter()
            .filter_map(|c| match c {
                MockCall::WriteMacroStream { macro_slot, .. } => Some(macro_slot.get()),
                _ => None,
            })
            .collect()
    }

    fn wrote_nothing(dev: &MockDevice) -> bool {
        !dev.calls().iter().any(|c| matches!(c, MockCall::WriteFullProfile { .. }))
    }

    #[test]
    fn no_macro_slot_takes_the_first_free_one() {
        let dev = pad(2);
        let (payload, code) =
            write(&dev, &def("New", XINPUT, "l3", 0), None, false, &ConfirmPolicy::Abort);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(
            (payload["macro_slot"].as_u64(), payload["name"].as_str()),
            (Some(2), Some("New"))
        );
        assert!(payload.get("dropped").is_none());
        assert_eq!(pages(&dev), [2]);
    }

    #[test]
    fn every_macro_slot_full_is_exit_2_with_no_write() {
        let dev = pad(4);
        let (payload, code) =
            write(&dev, &def("New", XINPUT, "l3", 0), None, false, &ConfirmPolicy::Force);
        assert_eq!(code, 2, "{payload}");
        assert!(payload["message"].as_str().unwrap().contains("All 4 macro slots"), "{payload}");
        assert!(wrote_nothing(&dev));
    }

    #[test]
    fn an_occupied_macro_slot_needs_force() {
        let dev = pad(2);
        let new = def("New", XINPUT, "l3", 0);
        let (payload, code) = write(&dev, &new, Some(1), false, &ConfirmPolicy::Abort);
        assert_eq!(code, 2, "{payload}");
        assert!(payload["message"].as_str().unwrap().contains("--force"), "{payload}");
        assert!(wrote_nothing(&dev));

        let (payload, code) = write(&dev, &new, Some(1), false, &ConfirmPolicy::Force);
        assert_eq!((code, payload["macro_slot"].as_u64()), (0, Some(1)), "{payload}");
        assert_eq!(pages(&dev), [1]);
    }

    #[test]
    fn another_modes_macro_needs_convert() {
        let dev = pad(2);
        let dinput = def("Combo", DINPUT, "r1", 0);
        let (payload, code) = write(&dev, &dinput, None, false, &ConfirmPolicy::Force);
        assert_eq!(code, 4, "{payload}");
        assert!(payload["message"].as_str().unwrap().contains("--convert"), "{payload}");
        assert!(wrote_nothing(&dev));

        // `r1` starts Held1, so the conversion drops it and the macro has no button.
        let (payload, code) = write(&dev, &dinput, None, true, &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["macro_slot"], 2);
        let dropped = payload["dropped"].as_array().unwrap();
        assert_eq!(dropped.len(), 1, "{payload}");
        assert_eq!(dropped[0]["item"], "r1");
        assert!(dropped[0]["reason"].as_str().unwrap().contains("Held1"), "{payload}");
        let bank = &dev.read_all_profiles().unwrap().raw_blobs[0];
        let held = Pro3.decode_macro_metadata(bank, slot1()).unwrap();
        let combo = held.iter().find(|m| m.macro_slot == Some(2)).unwrap();
        assert_eq!((combo.name.as_str(), combo.trigger_buttons.len()), ("Combo", 0));
    }

    #[test]
    fn a_switch_macro_converts_with_nothing_dropped() {
        let dev = pad(0);
        let switch = def("Hop", SWITCH, "l3", 0);
        let (payload, code) = write(&dev, &switch, Some(0), true, &ConfirmPolicy::Abort);
        assert_eq!(code, 0, "{payload}");
        assert_eq!(payload["dropped"], json!([]));
    }

    #[test]
    fn a_file_that_is_not_a_macro_is_exit_4() {
        let dev = pad(0);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("macro.json");
        for text in [None, Some("{"), Some("{\"name\": 3}")] {
            if let Some(text) = text {
                std::fs::write(&file, text).unwrap();
            }
            let req = MacroWrite {
                file: &file,
                mode: XINPUT,
                slot: slot1(),
                macro_slot: None,
                convert: false,
            };
            let (payload, code) = write_macro(&dev, &req, &ConfirmPolicy::Force);
            assert_eq!(code, 4, "{text:?}: {payload}");
        }
        assert!(wrote_nothing(&dev));
    }

    #[test]
    fn an_empty_profile_slot_is_exit_4() {
        let dev = pad(0);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("macro.json");
        let json = macro_to_json(&def("New", XINPUT, "l3", 0), "8bitdo-pro3");
        std::fs::write(&file, json.to_string()).unwrap();
        let slot = Slot::new(2).unwrap();
        let req = MacroWrite { file: &file, mode: XINPUT, slot, macro_slot: None, convert: false };
        let (payload, code) = write_macro(&dev, &req, &ConfirmPolicy::Force);
        assert_eq!(code, 4, "{payload}");
        assert!(wrote_nothing(&dev));
    }

    #[test]
    fn delete_asks_and_an_empty_macro_slot_writes_nothing() {
        let dev = pad(2);
        let m = |n| MacroSlot::new(n).unwrap();
        let (payload, code) = delete_macro(&dev, XINPUT, slot1(), m(3), &ConfirmPolicy::Abort);
        assert_eq!((code, payload["macro_slot"].as_u64()), (0, Some(3)), "{payload}");
        assert!(wrote_nothing(&dev));

        let (payload, code) = delete_macro(&dev, XINPUT, slot1(), m(1), &ConfirmPolicy::Abort);
        assert_eq!(code, 2, "{payload}");
        assert!(wrote_nothing(&dev));

        let (payload, code) = delete_macro(&dev, XINPUT, slot1(), m(1), &ConfirmPolicy::Force);
        assert_eq!(code, 0, "{payload}");
        let bank = &dev.read_all_profiles().unwrap().raw_blobs[0];
        let held = Pro3.decode_macro_metadata(bank, slot1()).unwrap();
        assert_eq!(held.iter().map(|h| h.macro_slot).collect::<Vec<_>>(), [Some(0)]);
    }

    #[test]
    fn the_payload_adds_its_fields() {
        let r = WriteResult::success(XINPUT, slot1(), "Macro written.");
        let (payload, code) = with_fields(&r, json!({ "macro_slot": 2, "name": "Dash" }));
        assert_eq!(
            (code, &payload["macro_slot"], &payload["name"]),
            (0, &json!(2), &json!("Dash"))
        );
        assert_eq!(payload["message"], "Macro written.");
    }
}
