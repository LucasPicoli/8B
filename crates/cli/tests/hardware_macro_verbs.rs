//! Hardware test of `write-macro` and `delete-macro`: requires a physical 8BitDo Pro 3
//! on USB. In each mode it runs the built binary on slot 2: it empties macro slots 1 to
//! 3, puts a neighbour in macro slot 0, writes a macro to the first free macro slot,
//! refuses to replace it without `--force`, replaces it with another mode's macro
//! through `--convert`, deletes it, and deletes the empty macro slot again. After each
//! verb the other banks are byte-identical, the written bank is the one before with
//! only the target macro changed, and the neighbour's steps still read back. The banks
//! and the two pages it wrote end as they began. Run with:
//!   `cargo test -p cli --features hardware --test hardware_macro_verbs -- --ignored`
#![cfg(feature = "hardware")]
#![allow(unreachable_pub, clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

#[path = "../../controller-core/tests/support/hw.rs"]
mod hw;
#[path = "../../controller-core/tests/pro3/seed.rs"]
mod seed;

use std::path::Path;
use std::process::{Command, Stdio};

use controller_core::device::ProtocolCodec as _;
use controller_core::devices::pro3::{Pro3, MODES};
use controller_core::model::macros::macro_to_json;
use controller_core::model::{MacroDefinition, MacroSlot, MacroStep, Mode, Slot};
use controller_core::protocol::wire_write::MACRO_CHUNK_LEN;
use controller_core::transport::{DeviceIo, HidrawDevice};
use serde_json::Value;

use hw::same_bank;

/// The profile slot the test writes macros into.
const SLOT: &str = "2";
/// The macro slot of the neighbour that must survive every verb.
const NEIGHBOUR: u8 = 0;
/// The first free macro slot once macro slots 1 to 3 are empty.
const TARGET: u8 = 1;

fn slot() -> Slot {
    Slot::new(2).unwrap()
}

fn macro_slot(n: u8) -> MacroSlot {
    MacroSlot::new(n).unwrap()
}

/// `n` steps that press a button, move the left stick and pull L2 fully, so they decode
/// the same and convert unchanged in every mode.
fn macro_def(mode: Mode, name: &str, trigger: &str, n: u8) -> MacroDefinition {
    let mut steps: Vec<MacroStep> = (0..n - 1)
        .map(|i| MacroStep {
            duration_ms: 40,
            pressed_buttons: vec![if i % 2 == 0 { "bottom face" } else { "right face" }.into()],
            left_stick_x: 20 * i,
            trigger_left: 255,
            ..MacroStep::default()
        })
        .collect();
    steps.push(MacroStep::default());
    MacroDefinition {
        name: name.into(),
        mode,
        trigger_buttons: vec![trigger.into()],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        steps,
        macro_slot: None,
    }
}

/// Runs the CLI with `args` and no terminal on stdin, so an unforced replace refuses.
/// Returns the exit code and the JSON it printed.
fn cli(args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_8bitdo-pro-3"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    // Shown only when the test fails.
    eprintln!("{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    let json = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), json)
}

/// Writes `def` as a `macro-v1` file in `dir` and returns its path.
fn file(dir: &Path, def: &MacroDefinition) -> String {
    let path = dir.join(format!("{}-{}.json", def.mode, def.name));
    let json = macro_to_json(def, &Pro3.default_profile(def.mode).device);
    std::fs::write(&path, json.to_string()).unwrap();
    path.to_string_lossy().into_owned()
}

/// The pages the test writes, as they were before it. Dropping the guard writes them
/// back, pass or fail.
struct Pages<'a> {
    dev: &'a HidrawDevice,
    saved: Vec<(Mode, u8, Vec<u8>)>,
}

/// Reads the written span of each page the test writes that a macro of the pad uses now.
fn save_pages(dev: &HidrawDevice) -> Pages<'_> {
    let read = dev.read_all_profiles().unwrap();
    let mut saved = Vec::new();
    for (mode, bank) in MODES.iter().zip(&read.raw_blobs) {
        for held in Pro3.decode_macro_metadata(bank, slot()).unwrap() {
            let m = held.macro_slot.unwrap();
            if [NEIGHBOUR, TARGET].contains(&m) && !held.steps.is_empty() {
                let read = dev.read_macro_stream(*mode, slot(), macro_slot(m), held.steps.len());
                let mut page = read.unwrap();
                page.resize(page.len().div_ceil(MACRO_CHUNK_LEN) * MACRO_CHUNK_LEN, 0);
                saved.push((*mode, m, page));
            }
        }
    }
    Pages { dev, saved }
}

impl Drop for Pages<'_> {
    fn drop(&mut self) {
        let dev = self.dev;
        let Ok(back_to) = dev.begin_write() else { return };
        for (mode, m, page) in &self.saved {
            let sent = dev.write_macro_stream(*mode, slot(), macro_slot(*m), page);
            let applied = sent.and_then(|()| dev.send_apply(*mode));
            if let Err(e) = applied {
                eprintln!("{mode} macro slot {m}: the page was not restored: {e}");
            }
        }
        if let Some(mode) = back_to {
            let _ = dev.end_write(mode);
        }
    }
}

/// After one verb on `bank`: the other banks are as `before`, `bank` is `before` with
/// only the target set to `want`, and the target's and the neighbour's steps read back.
fn check(
    dev: &HidrawDevice,
    before: &[Vec<u8>],
    bank: usize,
    mode: Mode,
    want: Option<&MacroDefinition>,
) {
    let now = dev.read_all_profiles().unwrap().raw_blobs;
    for (i, (was, is)) in before.iter().zip(&now).enumerate() {
        if i != bank {
            assert!(same_bank(was, is), "{mode}: bank {i} changed");
        }
    }
    let expected = Pro3.set_macro(&before[bank], slot(), macro_slot(TARGET), want).unwrap();
    assert!(same_bank(&expected, &now[bank]), "{mode}: more than the target changed");
    let read_steps = |m: u8, n: usize| {
        let stream = dev.read_macro_stream(mode, slot(), macro_slot(m), n).unwrap();
        Pro3.decode_macro_steps(&stream, n, mode).unwrap()
    };
    if let Some(def) = want {
        assert_eq!(read_steps(TARGET, def.steps.len()), def.steps, "{mode}: target steps");
    }
    let n = macro_def(mode, "Neighbour", "l1", 3);
    assert_eq!(read_steps(NEIGHBOUR, n.steps.len()), n.steps, "{mode}: neighbour steps");
}

#[test]
#[ignore = "writes and deletes macros on slot 2 of every mode of an attached 8BitDo Pro 3"]
fn write_macro_and_delete_macro_in_every_mode() {
    let dev = seed::pad();
    let pages = save_pages(&dev);
    let seed = seed::seed(&dev);
    let dir = tempfile::tempdir().unwrap();
    let banks = || dev.read_all_profiles().unwrap().raw_blobs;
    for (bank, &mode) in MODES.iter().enumerate() {
        let m = mode.as_str();
        let target = ["-m", m, "-s", SLOT];
        let ok = |args: &[&str]| {
            let (code, json) = cli(args);
            assert_eq!(code, 0, "{mode} {args:?}: {json}");
            json
        };
        for n in ["1", "2", "3"] {
            ok(&[&["delete-macro"][..], &target, &["--macro-slot", n, "--force"]].concat());
        }
        let neighbour = file(dir.path(), &macro_def(mode, "Neighbour", "l1", 3));
        let put = ["--macro-slot", "0", "--force", "-f", &neighbour];
        ok(&[&["write-macro"][..], &target, &put].concat());

        // The first free macro slot.
        let subject = macro_def(mode, "Subject", "r4", 6);
        let path = file(dir.path(), &subject);
        let before = banks();
        let json = ok(&[&["write-macro"][..], &target, &["-f", &path]].concat());
        assert_eq!(
            (json["macro_slot"].as_u64(), json["name"].as_str()),
            (Some(1), Some("Subject"))
        );
        check(&dev, &before, bank, mode, Some(&subject));

        // An occupied macro slot without --force.
        let before = banks();
        let (code, json) =
            cli(&[&["write-macro"][..], &target, &["--macro-slot", "1", "-f", &path]].concat());
        assert_eq!(code, 2, "{mode}: {json}");
        assert!(
            before.iter().zip(banks()).all(|(a, b)| same_bank(a, &b)),
            "{mode}: refused write wrote"
        );

        // Another mode's macro, converted.
        let other = MODES[(bank + 1) % MODES.len()];
        let moved = macro_def(other, "Moved", "r4", 4);
        let path = file(dir.path(), &moved);
        let before = banks();
        let args = ["--macro-slot", "1", "--convert", "--force", "-f", &path];
        let json = ok(&[&["write-macro"][..], &target, &args].concat());
        assert_eq!(json["dropped"], serde_json::json!([]), "{mode}");
        check(&dev, &before, bank, mode, Some(&MacroDefinition { mode, ..moved }));

        let before = banks();
        let json =
            ok(&[&["delete-macro"][..], &target, &["--macro-slot", "1", "--force"]].concat());
        assert_eq!(json["macro_slot"], 1);
        check(&dev, &before, bank, mode, None);

        // An empty macro slot: exit 0 with no write, so no --force is needed.
        let before = banks();
        ok(&[&["delete-macro"][..], &target, &["--macro-slot", "1"]].concat());
        assert!(
            before.iter().zip(banks()).all(|(a, b)| same_bank(a, &b)),
            "{mode}: empty delete wrote"
        );
    }
    drop(pages);
    drop(seed);
}
