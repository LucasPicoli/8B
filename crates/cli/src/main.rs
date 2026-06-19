//! CLI entry point for the 8BitDo Pro 3 configuration tool.
//!
//! Provides four subcommands mirroring the C++ `src/main.cpp` implementations:
//! `detect` (alias `readiness`), `read`, `dump`, and `read-macro`.
//!
//! Binary crates cannot expose a public API; suppress the lint that fires for
//! any `pub` item in a binary crate.
#![allow(unreachable_pub)]

pub(crate) mod commands;

use clap::{Parser, Subcommand};

use commands::{run_detect, run_dump, run_read, run_read_macro};
use controller_core::model::Mode;

/// 8BitDo Pro 3 CLI.
///
/// Exit codes:
///   0  Success
///   1  Connection failure (no device or USB error)
///   2  Usage error (invalid command or arguments)
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

/// Available CLI subcommands.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Detect whether a supported device is connected and report readiness.
    #[command(alias = "readiness")]
    Detect,

    /// Read all profiles from the device and report them as JSON.
    Read,

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
}

fn main() {
    let cli = Cli::parse();

    let code = match cli.command {
        Commands::Detect => run_detect(),
        Commands::Read => run_read(),
        Commands::Dump { output_dir } => run_dump(&output_dir),
        Commands::ReadMacro { mode, slot, output_dir } => {
            run_read_macro(mode, slot, output_dir.as_deref())
        }
    };

    std::process::exit(code);
}
