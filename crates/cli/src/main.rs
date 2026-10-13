//! CLI entry point for the 8BitDo Pro 3 configuration tool.
//!
//! Subcommands: `detect` (alias `readiness`), `read`,
//! `export`, `dump`, `read-macro`, the write verbs `upload`, `deactivate`, `remap`
//! and `set`, and the macro write verbs `write-macro` and `delete-macro`. The `dev` group adds tools for
//! reverse engineering a controller the app does not support yet.
//!
//! Binary crates cannot expose a public API; suppress the lint that fires for
//! any `pub` item in a binary crate.
#![allow(unreachable_pub)]

pub(crate) mod commands;
pub(crate) mod dev;
pub(crate) mod export;
pub(crate) mod macros;
pub(crate) mod upload;
pub(crate) mod write;

use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{ArgAction, Args, CommandFactory as _, Parser, Subcommand};

use commands::{run_detect, run_dump, run_read, run_read_macro};
use controller_core::devices;
use controller_core::model::{MacroSlot, Mode, Slot};
use export::run_export;
use macros::{run_delete_macro, run_write_macro, MacroWrite};
use upload::{run_upload, Upload};
use write::run_write;

/// 8BitDo Pro 3 CLI.
///
/// Exit codes:
///   0  Success
///   1  Connection failure (no device or USB error)
///   2  Usage error (invalid arguments), a write to an occupied slot or macro
///      slot refused without --force, or a write-macro with every macro slot full
///   3  Timeout (device disconnected mid-transfer)
///   4  Validation failure (profile or macro schema/semantic check failed)
///   5  Export failure (could not write files to disk, or a file exists and
///      --overwrite is off)
///   6  Write failure (profile write to device failed)
#[derive(Debug, Parser)]
#[command(name = "8bitdo-pro-3", version, about, verbatim_doc_comment)]
struct Cli {
    /// Log to stderr: -v for debug, -vv for trace. `RUST_LOG` overrides it.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Commands,
}

/// The `dev` subcommands.
#[derive(Debug, Subcommand)]
enum DevCommand {
    /// List every hidraw node: bus, USB id, interface, report descriptor size and name.
    List {
        /// Also print each report descriptor in hex.
        #[arg(long)]
        descriptors: bool,
    },
    /// Write raw bytes to a hidraw node and print every report read back.
    ///
    /// Sends exactly the bytes given. Unknown commands can change or erase a
    /// controller's flash: read before you write, and keep a dump.
    Send {
        /// The node, such as /dev/hidraw4.
        node: PathBuf,
        /// Hex bytes: `81 04 00 01`, `81040001` or `0x81,0x04`.
        #[arg(required = true, num_args = 1..)]
        bytes: Vec<String>,
        /// Zero-pad the packet to this many bytes, such as 64.
        #[arg(long)]
        pad: Option<usize>,
        /// How long to collect reports after the send, in milliseconds.
        #[arg(long, default_value_t = 500)]
        wait: u64,
        /// Print only reports that start with these hex bytes, such as `02 04`. Hides
        /// the gamepad's input reports.
        #[arg(long = "match", value_name = "HEX")]
        matching: Option<String>,
    },
    /// Print each run of bytes that differs between two dump files.
    Diff {
        /// The first dump.
        a: PathBuf,
        /// The second dump.
        b: PathBuf,
    },
}

/// Where a write goes, shared by every write verb.
#[derive(Debug, Args)]
struct Target {
    /// Target mode, by the id the controller's description gives it, such as xinput.
    #[arg(short, long, value_parser = parse_mode)]
    mode: Mode,
    /// Target slot, from 1.
    #[arg(short, long, value_parser = parse_slot)]
    slot: Slot,
    /// Overwrite an occupied slot or macro slot without asking. Only upload, deactivate,
    /// write-macro and delete-macro ask: set and remap edit the profile the slot holds.
    #[arg(long)]
    force: bool,
}

/// Parses `<pointer>=<value>` for `set`, the value as JSON.
fn parse_change(s: &str) -> Result<(String, serde_json::Value), String> {
    let (pointer, value) =
        s.split_once('=').ok_or_else(|| format!("'{s}' is not <pointer>=<value>"))?;
    if !pointer.starts_with('/') {
        return Err(format!("'{pointer}' is not a JSON pointer, such as /vibration/left_level"));
    }
    let value = serde_json::from_str(value)
        .map_err(|_| format!("'{value}' is not a number or true/false"))?;
    Ok((pointer.to_owned(), value))
}

/// The first pointer that `changes` sets twice, if any.
fn repeated_pointer(changes: &[(String, serde_json::Value)]) -> Option<&str> {
    changes
        .iter()
        .enumerate()
        .find(|(i, (p, _))| changes.iter().take(*i).any(|(q, _)| q == p))
        .map(|(_, (p, _))| p.as_str())
}

/// Parses a mode that a built-in model declares, so a typo is a usage error before any
/// device access. The model of the attached pad checks it again.
fn parse_mode(s: &str) -> Result<Mode, String> {
    let mut known: Vec<Mode> = Vec::new();
    for mode in devices::descriptions().flat_map(|d| d.modes.iter().map(|m| m.id)) {
        if !known.contains(&mode) {
            known.push(mode);
        }
    }
    known.iter().copied().find(|m| m.as_str() == s).ok_or_else(|| {
        let ids: Vec<&str> = known.iter().map(Mode::as_str).collect();
        format!("unknown mode '{s}' (one of {})", ids.join(", "))
    })
}

/// Parses a 1-based slot for clap, up to the largest slot count of a built-in model.
/// The model of the attached pad checks it against its own count again.
fn parse_slot(s: &str) -> Result<Slot, String> {
    let most = devices::descriptions().map(|d| d.slot_count).max().unwrap_or(0);
    s.parse::<u8>()
        .map_err(|e| e.to_string())
        .and_then(|n| Slot::new(n).and_then(|slot| slot.check(most)).map_err(|e| e.to_string()))
}

/// Parses a 0-based macro slot for clap, below the largest macro slot count of a
/// built-in model. The model of the attached pad checks it against its own count again.
fn parse_macro_slot(s: &str) -> Result<MacroSlot, String> {
    let count = devices::descriptions().map(|d| d.macro_slot_count).max().unwrap_or(0);
    let n = s.parse::<u8>().map_err(|e| e.to_string())?;
    match count.checked_sub(1) {
        Some(last) if n <= last => MacroSlot::new(n).map_err(|e| e.to_string()),
        Some(last) => Err(format!("macro slot {n} out of range (0 to {last})")),
        None => Err("no supported controller has macro slots".to_owned()),
    }
}

/// Available CLI subcommands.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Detect whether a supported device is connected and report readiness.
    #[command(alias = "readiness")]
    Detect,

    /// Read all profiles from the device and report them as JSON.
    Read,

    /// Save every profile of every mode as canonical JSON (re-uploadable), one file per
    /// slot with the slot's macros in its `macros` list.
    Export {
        /// Output directory.
        #[arg(short, long, default_value = "exports")]
        output_dir: PathBuf,
        /// Replace files that already exist. Without it, an existing file stops the
        /// export before anything is written, with exit 5.
        #[arg(long)]
        overwrite: bool,
    },

    /// Dump raw profile blobs to disk for diagnostics.
    Dump {
        /// Directory to write raw blobs into.
        output_dir: String,
    },

    /// Read macros from a profile slot and report them as JSON.
    #[command(name = "read-macro")]
    ReadMacro {
        /// Mode, by the id the controller's description gives it, such as xinput.
        #[arg(value_parser = parse_mode)]
        mode: Mode,
        /// Profile slot, from 1.
        #[arg(value_parser = parse_slot)]
        slot: Slot,
        /// Optional directory to write per-macro JSON files.
        #[arg(long = "output-dir")]
        output_dir: Option<String>,
    },

    /// Write a canonical profile JSON file into a slot. Keeps the slot's macros, unless
    /// --with-macros is given.
    ///
    /// A `warnings` list names each macro whose stick moves the profile's stick dead zone
    /// stops. The write still goes ahead.
    Upload {
        #[command(flatten)]
        target: Target,
        /// Path to the canonical profile JSON.
        #[arg(short, long)]
        file: PathBuf,
        /// Replace the slot's macros with the profile's `macros`, in the same write. Each
        /// macro goes in its `macro_slot`, and the other macro slots are emptied; an
        /// empty list clears them all. A profile with no `macros` key takes the
        /// `macro-v1` files its `macro_refs` name instead, read from the profile's
        /// folder, each in the macro slot its name gives
        /// (`<mode>-slot<N>-macro<M>-<name>.json`).
        #[arg(long)]
        with_macros: bool,
    },

    /// Write a `macro-v1` file into a macro slot of an occupied slot. The slot's other
    /// macros and its profile stay as they are.
    ///
    /// A `warnings` list names each stick the macro moves that the slot's stick dead zone
    /// stops. The write still goes ahead.
    ///
    /// Example: `write-macro -m xinput -s 1 -f xinput-slot1-macro0-Dash.json`.
    #[command(name = "write-macro")]
    WriteMacro {
        #[command(flatten)]
        target: Target,
        /// Macro slot, from 0. Omitted, the first free one; exit 2 when none is free.
        #[arg(long, value_parser = parse_macro_slot)]
        macro_slot: Option<MacroSlot>,
        /// Convert a macro made for another mode. Each button that cannot start it in
        /// the target slot, and each trigger glide an on/off mode cannot play, is dropped
        /// and listed under `dropped`. Without it, another mode's macro is exit 4.
        #[arg(long)]
        convert: bool,
        /// Path to the `macro-v1` JSON.
        #[arg(short, long)]
        file: PathBuf,
    },

    /// Delete the macro in a macro slot. An empty macro slot is exit 0 with no write.
    #[command(name = "delete-macro")]
    DeleteMacro {
        #[command(flatten)]
        target: Target,
        /// Macro slot, from 0.
        #[arg(long, value_parser = parse_macro_slot)]
        macro_slot: MacroSlot,
    },

    /// Clear a slot.
    Deactivate {
        #[command(flatten)]
        target: Target,
    },

    /// Map one button to another output, or to `disabled`.
    Remap {
        #[command(flatten)]
        target: Target,
        /// Source button name (for example `right face`).
        #[arg(long)]
        source: String,
        /// Output name (for example `bottom face` or `disabled`).
        #[arg(long = "target")]
        output: String,
    },

    /// Set settings of an occupied slot by their JSON pointer, as the controller's
    /// description names them. Unnamed settings keep their value.
    ///
    /// Example: `set -m xinput -s 1 /vibration/left_level=3 /sticks/swap_sticks=true`.
    Set {
        #[command(flatten)]
        target: Target,
        /// One or more `<pointer>=<value>`. The value is a number or true/false.
        #[arg(required = true, num_args = 1.., value_parser = parse_change)]
        changes: Vec<(String, serde_json::Value)>,
    },

    /// Tools for reverse engineering a new controller. Plain-text output.
    #[command(subcommand)]
    Dev(DevCommand),
}

/// The log filter for `-v` flags: `warn`, then `debug`, then `trace`. A non-empty
/// `RUST_LOG` wins over the flags.
fn log_spec(verbose: u8, rust_log: Option<&str>) -> String {
    rust_log.filter(|s| !s.is_empty()).map_or_else(
        || (["warn", "debug", "trace"].get(usize::from(verbose)).unwrap_or(&"trace")).to_string(),
        str::to_owned,
    )
}

fn main() {
    let cli = Cli::parse();
    if let Commands::Set { changes, .. } = &cli.command {
        if let Some(pointer) = repeated_pointer(changes) {
            let message = format!("'{pointer}' is set twice; give each setting once");
            Cli::command().error(ErrorKind::ArgumentConflict, message).exit();
        }
    }
    env_logger::Builder::new()
        .parse_filters(&log_spec(cli.verbose, std::env::var("RUST_LOG").ok().as_deref()))
        .init();

    let code = match cli.command {
        Commands::Detect => run_detect(),
        Commands::Read => run_read(),
        Commands::Export { output_dir, overwrite } => run_export(&output_dir, overwrite),
        Commands::Dump { output_dir } => run_dump(&output_dir),
        Commands::ReadMacro { mode, slot, output_dir } => {
            run_read_macro(mode, slot.get(), output_dir.as_deref())
        }
        Commands::Dev(DevCommand::List { descriptors }) => dev::run_list(descriptors),
        Commands::Dev(DevCommand::Send { node, bytes, pad, wait, matching }) => {
            dev::run_send(&dev::Send {
                node: &node,
                bytes: &bytes,
                pad,
                wait_ms: wait,
                matching: matching.as_deref(),
            })
        }
        Commands::Dev(DevCommand::Diff { a, b }) => dev::run_diff(&a, &b),
        Commands::Upload { target: t, file, with_macros } => {
            run_upload(&Upload { file: &file, mode: t.mode, slot: t.slot, with_macros }, t.force)
        }
        Commands::WriteMacro { target: t, macro_slot, convert, file } => run_write_macro(
            &MacroWrite { file: &file, mode: t.mode, slot: t.slot, macro_slot, convert },
            t.force,
        ),
        Commands::DeleteMacro { target: t, macro_slot } => {
            run_delete_macro(t.mode, t.slot, macro_slot, t.force)
        }
        Commands::Deactivate { target: t } => {
            run_write(t.mode, t.slot, t.force, &[], |o, p| o.deactivate_slot(t.mode, t.slot, p))
        }
        Commands::Remap { target: t, source, output } => run_write(
            t.mode,
            t.slot,
            t.force,
            &[("source", &source), ("target", &output)],
            |o, _| o.remap_button(t.mode, t.slot, &source, &output),
        ),
        Commands::Set { target: t, changes } => {
            let changes: Vec<(&str, serde_json::Value)> =
                changes.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            run_write(t.mode, t.slot, t.force, &[], |o, _| {
                o.patch_settings(t.mode, t.slot, &changes)
            })
        }
    };

    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_takes_pointers_with_number_or_flag_values() {
        assert_eq!(
            parse_change("/vibration/left_level=3"),
            Ok(("/vibration/left_level".to_owned(), 3.into()))
        );
        assert_eq!(parse_change("/lights/on=true"), Ok(("/lights/on".to_owned(), true.into())));
        assert!(parse_change("vibration=3").is_err(), "a pointer starts with a slash");
        assert!(parse_change("/a").is_err());
        assert!(parse_change("/a=loud").is_err());
        let cli = Cli::parse_from(["8b", "set", "-m", "xinput", "-s", "1", "/a=1", "/b=false"]);
        assert!(matches!(cli.command, Commands::Set { changes, .. } if changes.len() == 2));
    }

    #[test]
    fn set_finds_a_pointer_given_twice() {
        let change = |s: &str| (s.to_owned(), serde_json::Value::from(1));
        let once = [change("/a"), change("/b")];
        assert_eq!(repeated_pointer(&once), None);
        let twice = [change("/a"), change("/b"), change("/a")];
        assert_eq!(repeated_pointer(&twice), Some("/a"));
    }

    #[test]
    fn a_mode_or_slot_no_model_has_is_a_usage_error() {
        assert!(parse_mode("xinput").is_ok());
        assert_eq!(
            parse_mode("foo"),
            Err("unknown mode 'foo' (one of xinput, switch, dinput)".to_owned())
        );
        assert!(parse_slot("3").is_ok());
        for bad in ["0", "4", "x"] {
            assert!(parse_slot(bad).is_err(), "{bad}");
        }
        for args in [
            ["8b", "deactivate", "-m", "foo", "-s", "1"],
            ["8b", "deactivate", "-m", "xinput", "-s", "4"],
        ] {
            let code = Cli::try_parse_from(args).err().map(|e| e.exit_code());
            assert_eq!(code, Some(2), "{args:?}");
        }
        assert!(Cli::try_parse_from(["8b", "read-macro", "foo", "1"]).is_err());
        assert!(Cli::try_parse_from(["8b", "read-macro", "xinput", "4"]).is_err());
    }

    #[test]
    fn macro_verbs_take_a_target_and_a_macro_slot() {
        let cli = Cli::parse_from(["8b", "write-macro", "-m", "dinput", "-s", "2", "-f", "m.json"]);
        assert!(matches!(
            cli.command,
            Commands::WriteMacro { macro_slot: None, convert: false, .. }
        ));
        let cli = Cli::parse_from([
            "8b",
            "write-macro",
            "-m",
            "xinput",
            "-s",
            "1",
            "--macro-slot",
            "3",
            "--convert",
            "--force",
            "-f",
            "m.json",
        ]);
        assert!(matches!(
            cli.command,
            Commands::WriteMacro { macro_slot: Some(m), convert: true, target: Target { force: true, .. }, .. }
                if m.get() == 3
        ));
        let cli =
            Cli::parse_from(["8b", "delete-macro", "-m", "switch", "-s", "3", "--macro-slot", "0"]);
        assert!(
            matches!(cli.command, Commands::DeleteMacro { macro_slot, .. } if macro_slot.get() == 0)
        );

        assert!(parse_macro_slot("0").is_ok());
        assert_eq!(parse_macro_slot("4"), Err("macro slot 4 out of range (0 to 3)".to_owned()));
        for args in [
            &["8b", "delete-macro", "-m", "xinput", "-s", "1"][..],
            &["8b", "delete-macro", "-m", "xinput", "-s", "1", "--macro-slot", "4"],
            &["8b", "delete-macro", "-m", "foo", "-s", "1", "--macro-slot", "0"],
            &["8b", "write-macro", "-m", "xinput", "-s", "4", "-f", "m.json"],
            &["8b", "write-macro", "-m", "xinput", "-s", "1", "--macro-slot", "x", "-f", "m"],
            &["8b", "write-macro", "-m", "xinput", "-s", "1"],
        ] {
            let code = Cli::try_parse_from(args).err().map(|e| e.exit_code());
            assert_eq!(code, Some(2), "{args:?}");
        }
    }

    #[test]
    fn verbose_flags_raise_the_level() {
        assert_eq!(log_spec(0, None), "warn");
        assert_eq!(log_spec(1, None), "debug");
        assert_eq!(log_spec(2, None), "trace");
        assert_eq!(log_spec(5, None), "trace");
    }

    #[test]
    fn rust_log_wins_over_the_flags() {
        assert_eq!(log_spec(2, Some("controller_core=info")), "controller_core=info");
        assert_eq!(log_spec(1, Some("")), "debug");
    }

    #[test]
    fn verbose_counts_repeats_before_or_after_the_verb() {
        assert_eq!(Cli::parse_from(["8b", "-vv", "detect"]).verbose, 2);
        assert_eq!(Cli::parse_from(["8b", "detect", "-v"]).verbose, 1);
    }
}
