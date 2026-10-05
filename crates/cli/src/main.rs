//! CLI entry point for the 8BitDo Pro 3 configuration tool.
//!
//! Mirrors the C++ `src/main.cpp` subcommands: `detect` (alias `readiness`), `read`,
//! `export`, `dump`, `read-macro`, and the write verbs `upload`, `deactivate`, `remap`,
//! `patch-sticks`, `patch-triggers` and `patch-vibration`.
//!
//! Binary crates cannot expose a public API; suppress the lint that fires for
//! any `pub` item in a binary crate.
#![allow(unreachable_pub)]

pub(crate) mod commands;
pub(crate) mod export;
pub(crate) mod write;

use std::path::PathBuf;

use clap::builder::BoolishValueParser;
use clap::{Args, Parser, Subcommand};

use commands::{run_detect, run_dump, run_read, run_read_macro};
use controller_core::model::{Mode, Slot};
use controller_core::orchestrator::{StickPatch, TriggerPatch};
use export::run_export;
use write::{run_upload, run_write};

/// Exit code for a usage error, as clap uses it.
const EXIT_USAGE: i32 = 2;

/// 8BitDo Pro 3 CLI.
///
/// Exit codes:
///   0  Success
///   1  Connection failure (no device or USB error)
///   2  Usage error (invalid arguments, or an overwrite refused without --force)
///   3  Timeout (device disconnected mid-transfer)
///   4  Validation failure (profile schema/semantic check failed)
///   5  Export failure (could not write files to disk)
///   6  Write failure (profile write to device failed)
#[derive(Debug, Parser)]
#[command(name = "8bitdo-pro-3", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Where a write goes, shared by every write verb.
#[derive(Debug, Args)]
struct Target {
    /// Target mode: xinput, switch or dinput.
    #[arg(short, long)]
    mode: Mode,
    /// Target slot (1-3).
    #[arg(short, long, value_parser = parse_slot)]
    slot: Slot,
    /// Overwrite an occupied slot without asking.
    #[arg(long)]
    force: bool,
}

/// Parses a 1-based slot for clap.
fn parse_slot(s: &str) -> Result<Slot, String> {
    s.parse::<u8>().map_err(|e| e.to_string()).and_then(|n| Slot::new(n).map_err(|e| e.to_string()))
}

/// Available CLI subcommands.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Detect whether a supported device is connected and report readiness.
    #[command(alias = "readiness")]
    Detect,

    /// Read all profiles from the device and report them as JSON.
    Read,

    /// Save every profile of every mode as canonical JSON (re-uploadable).
    Export {
        /// Output directory.
        #[arg(short, long, default_value = "exports")]
        output_dir: PathBuf,
        /// Replace files that already exist.
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
        /// Mode: xinput or switch (dinput not supported for macros).
        mode: Mode,
        /// Profile slot (1–3).
        slot: u8,
        /// Optional directory to write per-macro JSON files.
        #[arg(long = "output-dir")]
        output_dir: Option<String>,
    },

    /// Write a canonical profile JSON file into a slot. Keeps the slot's macros.
    Upload {
        #[command(flatten)]
        target: Target,
        /// Path to the canonical profile JSON.
        #[arg(short, long)]
        file: PathBuf,
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

    /// Change stick settings in an occupied slot. Unset options keep their value.
    #[command(name = "patch-sticks")]
    PatchSticks {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        sticks: StickArgs,
    },

    /// Change trigger settings in an occupied slot. Unset options keep their value.
    #[command(name = "patch-triggers")]
    PatchTriggers {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        triggers: TriggerArgs,
    },

    /// Set both vibration levels of an occupied slot.
    #[command(name = "patch-vibration")]
    PatchVibration {
        #[command(flatten)]
        target: Target,
        /// Left motor level (0-5).
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=5))]
        left: u8,
        /// Right motor level (0-5).
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=5))]
        right: u8,
    },
}

/// `patch-sticks` options.
#[derive(Debug, Args)]
struct StickArgs {
    /// Left stick min (dead zone) percent.
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    left_min: Option<i32>,
    /// Left stick max (range) percent.
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    left_max: Option<i32>,
    /// Right stick min (dead zone) percent.
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    right_min: Option<i32>,
    /// Right stick max (range) percent.
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    right_max: Option<i32>,
    /// Invert left stick X (true/false).
    #[arg(long, value_parser = BoolishValueParser::new())]
    invert_left_x: Option<bool>,
    /// Invert left stick Y (true/false).
    #[arg(long, value_parser = BoolishValueParser::new())]
    invert_left_y: Option<bool>,
    /// Invert right stick X (true/false).
    #[arg(long, value_parser = BoolishValueParser::new())]
    invert_right_x: Option<bool>,
    /// Invert right stick Y (true/false).
    #[arg(long, value_parser = BoolishValueParser::new())]
    invert_right_y: Option<bool>,
    /// Swap the two sticks (true/false).
    #[arg(long, value_parser = BoolishValueParser::new())]
    swap_sticks: Option<bool>,
    /// Swap the D-pad with the left stick (true/false).
    #[arg(long, value_parser = BoolishValueParser::new())]
    swap_dpad: Option<bool>,
}

impl From<StickArgs> for StickPatch {
    fn from(a: StickArgs) -> Self {
        Self {
            left_min_pct: a.left_min,
            left_max_pct: a.left_max,
            right_min_pct: a.right_min,
            right_max_pct: a.right_max,
            invert_left_x: a.invert_left_x,
            invert_left_y: a.invert_left_y,
            invert_right_x: a.invert_right_x,
            invert_right_y: a.invert_right_y,
            swap_sticks: a.swap_sticks,
            swap_dpad_with_left_stick: a.swap_dpad,
        }
    }
}

/// `patch-triggers` options.
#[derive(Debug, Args)]
struct TriggerArgs {
    /// Left trigger min percent (xinput, dinput).
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    left_min: Option<i32>,
    /// Left trigger max percent (xinput, dinput).
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    left_max: Option<i32>,
    /// Right trigger min percent (xinput, dinput).
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    right_min: Option<i32>,
    /// Right trigger max percent (xinput, dinput).
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    right_max: Option<i32>,
    /// Left trigger threshold percent (switch).
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    left_threshold: Option<i32>,
    /// Right trigger threshold percent (switch).
    #[arg(long, value_parser = clap::value_parser!(i32).range(0..=100))]
    right_threshold: Option<i32>,
    /// Swap the two triggers (true/false).
    #[arg(long, value_parser = BoolishValueParser::new())]
    swap_triggers: Option<bool>,
}

impl From<TriggerArgs> for TriggerPatch {
    fn from(a: TriggerArgs) -> Self {
        Self {
            left_min_pct: a.left_min,
            left_max_pct: a.left_max,
            right_min_pct: a.right_min,
            right_max_pct: a.right_max,
            left_threshold_pct: a.left_threshold,
            right_threshold_pct: a.right_threshold,
            swap_triggers: a.swap_triggers,
        }
    }
}

/// Prints a usage error for a patch with no option set.
fn no_option(verb: &str, example: &str) -> i32 {
    eprintln!("{verb} requires at least one option (for example {example}).");
    EXIT_USAGE
}

fn main() {
    let cli = Cli::parse();

    let code = match cli.command {
        Commands::Detect => run_detect(),
        Commands::Read => run_read(),
        Commands::Export { output_dir, overwrite } => run_export(&output_dir, overwrite),
        Commands::Dump { output_dir } => run_dump(&output_dir),
        Commands::ReadMacro { mode, slot, output_dir } => {
            run_read_macro(mode, slot, output_dir.as_deref())
        }
        Commands::Upload { target: t, file } => run_upload(&file, t.mode, t.slot, t.force),
        Commands::Deactivate { target: t } => {
            run_write(t.force, &[], |o, p| o.deactivate_slot(t.mode, t.slot, p))
        }
        Commands::Remap { target: t, source, output } => {
            run_write(t.force, &[("source", &source), ("target", &output)], |o, p| {
                o.remap_button(t.mode, t.slot, &source, &output, p)
            })
        }
        Commands::PatchSticks { target: t, sticks } => {
            let patch = StickPatch::from(sticks);
            if patch == StickPatch::default() {
                no_option("patch-sticks", "--left-min 10")
            } else {
                run_write(t.force, &[], |o, p| o.patch_sticks(t.mode, t.slot, &patch, p))
            }
        }
        Commands::PatchTriggers { target: t, triggers } => {
            let patch = TriggerPatch::from(triggers);
            if patch == TriggerPatch::default() {
                no_option("patch-triggers", "--left-min 5 or --left-threshold 30")
            } else {
                run_write(t.force, &[], |o, p| o.patch_triggers(t.mode, t.slot, &patch, p))
            }
        }
        Commands::PatchVibration { target: t, left, right } => {
            run_write(t.force, &[], |o, p| o.patch_vibration(t.mode, t.slot, left, right, p))
        }
    };

    std::process::exit(code);
}
