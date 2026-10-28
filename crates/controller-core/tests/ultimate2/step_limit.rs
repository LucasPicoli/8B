//! The Ultimate 2's own step limit for `DInput` slot 3, macro 1: its firmware erases a
//! longer macro there at the next `DInput` start-up (`step_probe::record_probe`), so a
//! write refuses one. Every other macro slot keeps the controller's own limit.

use crate::fixture::{profile_json, SLOT};
use controller_core::devices::model_named;
use controller_core::devices::pro3::{DINPUT, XINPUT};
use controller_core::model::{MacroDefinition, MacroSlot, MacroStep, ProfileReadResult, Slot};
use controller_core::orchestrator::{MacroEdit, ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::validation::slot_step_error;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::mock::{MockCall, MockDevice};

/// The limit the description sets for `DInput` slot 3, macro 1.
const SLOT_3_MACRO_1: usize = 307;

/// A macro of `steps` plain steps in `DInput` macro slot `n`, started by X.
fn long(steps: usize, n: u8) -> MacroDefinition {
    MacroDefinition {
        name: "Long".into(),
        mode: DINPUT,
        trigger_buttons: vec!["left face".into()],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        macro_slot: Some(n),
        steps: vec![MacroStep { duration_ms: 50, ..MacroStep::default() }; steps],
    }
}

#[test]
fn only_dinput_slot_3_macro_1_has_a_lower_step_limit() {
    let d = model_named("Ultimate 2").unwrap().description().unwrap();
    let top = d.limits.macro_steps.max;
    assert_eq!(d.macro_steps_max(DINPUT, 3, 0), i32::try_from(SLOT_3_MACRO_1).unwrap());
    for (mode, slot, n) in [(DINPUT, 3, 1), (DINPUT, 2, 0), (XINPUT, 3, 0)] {
        assert_eq!(d.macro_steps_max(mode, slot, n), top, "{mode} slot {slot} macro {n}");
    }
    assert!(slot_step_error(d, DINPUT, 3, 0, &long(SLOT_3_MACRO_1, 0)).is_none());
    let refused = slot_step_error(d, DINPUT, 3, 0, &long(SLOT_3_MACRO_1 + 1, 0)).unwrap();
    assert_eq!(refused.path, "/steps");
    assert!(refused.reason.to_string().contains("at most 307"), "{}", refused.reason);
    assert!(slot_step_error(d, DINPUT, 2, 0, &long(SLOT_3_MACRO_1 + 1, 0)).is_none());
}

#[test]
fn a_write_refuses_a_longer_macro_1_in_dinput_slot_3_before_it_sends_anything() {
    let model = model_named("Ultimate 2").unwrap();
    // A DInput bank the pad never saved: erased flash with the mode byte.
    let mut dinput = vec![0xff; model.blob_size()];
    dinput[0x10] = 1;
    dinput[0x11] = 0;
    let dev = MockDevice::new().with_model(model).with_current_mode(DINPUT).with_profiles(
        ProfileReadResult { raw_blobs: vec![Vec::new(), dinput], ..Default::default() },
    );
    let dir = tempfile::tempdir().unwrap();
    let def = long(SLOT_3_MACRO_1 + 1, 0);
    let op = WriteOp::Upload {
        profile: Some(profile_json(DINPUT)),
        drop_macros: Vec::new(),
        macro_edits: vec![MacroEdit::Put {
            macro_slot: MacroSlot::new(0).unwrap(),
            definition: def,
        }],
    };
    let job = WriteJob { mode: DINPUT, slot: Slot::new(SLOT).unwrap(), op };
    let writer = ProfileWriteOrchestrator::new(&dev, model, dir.path());
    let result = writer.write_slots(&[job], &ConfirmPolicy::Force).pop().unwrap();
    assert!(!result.success && result.message.contains("at most 307"), "{}", result.message);
    let sent = dev.calls().into_iter().filter(|c| {
        matches!(c, MockCall::WriteFullProfile { .. } | MockCall::WriteMacroStream { .. })
    });
    assert_eq!(sent.count(), 0, "nothing reached the pad");
}
