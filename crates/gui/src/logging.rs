//! The GUI log: `debug` lines in a file under the state dir, `warn` lines on stderr.
//!
//! A user who starts 8B from the menu never sees stderr, so the file is the record a
//! bug report can carry. `$HOME` is written as `~` in every line, and the file stops
//! at [`LOG_LIMIT`] bytes. A file that cannot be opened leaves stderr alone to log.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use env_logger::fmt::Formatter;
use env_logger::{Builder, Logger, Target};
use log::{LevelFilter, Log, Metadata, Record};

use crate::writes::state_dir;

/// The log of the current run, in the state dir.
const LOG_NAME: &str = "8b.log";
/// The log of the run before, kept for a report about a crash.
const PREV_NAME: &str = "8b.prev.log";
/// Bytes after which the file takes no more lines (2 MB).
const LOG_LIMIT: u64 = 2 * 1024 * 1024;
/// The last line of a full file.
const FULL_LINE: &str = "log full: no more lines are written\n";
/// The file records 8B and the core at `debug` whatever stderr shows. Other crates
/// stop at `info`: the Wayland client logs a line per protocol event at `debug`.
const FILE_SPEC: &str = "info,8b=debug,controller_core=debug";
/// Stderr shows this level unless `RUST_LOG` says otherwise.
const STDERR_SPEC: &str = "warn";

/// Starts logging for the run: the header line, then the file and stderr sinks.
/// `RUST_LOG`, when set, replaces both levels.
pub fn init(sandboxed: bool) {
    let home = std::env::var("HOME").ok();
    let rust_log = std::env::var("RUST_LOG").ok().filter(|s| !s.is_empty());
    let dir = state_dir(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"));
    let opened = rotate(&dir).map(|file| Capped::new(file, LOG_LIMIT));
    let (file, failed) = match opened {
        Ok(capped) => {
            (Some(sink(rust_log.as_deref().unwrap_or(FILE_SPEC), home.as_deref(), capped)), None)
        }
        Err(e) => (None, Some(e)),
    };
    let stderr = Builder::new()
        .parse_filters(rust_log.as_deref().unwrap_or(STDERR_SPEC))
        .format(formatter(home))
        .build();
    let both = Both { file, stderr };
    log::set_max_level(both.max_level());
    // A second logger in the same process is a bug in a test, not a reason to stop.
    let _ = log::set_boxed_logger(Box::new(both));
    log::info!(
        "8b {}, build {}, sandboxed {sandboxed}",
        env!("CARGO_PKG_VERSION"),
        build_kind(sandboxed, std::env::var_os("APPIMAGE").is_some())
    );
    if let Some(e) = failed {
        log::warn!("cannot open the log file, logging to stderr only: {e}");
    }
}

/// Which package the run came from, for the header line.
const fn build_kind(sandboxed: bool, appimage: bool) -> &'static str {
    if sandboxed {
        "flatpak"
    } else if appimage {
        "appimage"
    } else {
        "native"
    }
}

/// Builds the file sink: `spec` as the level, `~` for home, bytes into `out`.
fn sink(spec: &str, home: Option<&str>, out: impl Write + Send + 'static) -> Logger {
    Builder::new()
        .parse_filters(spec)
        .format(formatter(home.map(str::to_owned)))
        .target(Target::Pipe(Box::new(out)))
        .build()
}

/// The line format of both sinks: time, level, source and text, with home hidden.
fn formatter(
    home: Option<String>,
) -> impl Fn(&mut Formatter, &Record<'_>) -> io::Result<()> + Send + Sync + 'static {
    move |buf, record| {
        let text = hide_home(&record.args().to_string(), home.as_deref());
        writeln!(
            buf,
            "{} {:<5} {} {text}",
            buf.timestamp_seconds(),
            record.level(),
            record.target()
        )
    }
}

/// `text` with `home` written as `~`, where it ends a path component: `/home/ada/x`
/// and `/home/ada` change, `/home/adam` does not. An empty home or `/` hides nothing.
fn hide_home(text: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|h| h.len() > 1) else {
        return text.to_owned();
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(home) {
        let (before, tail) = rest.split_at(at);
        let after = tail.get(home.len()..).unwrap_or_default();
        out.push_str(before);
        let in_name = after.chars().next().is_some_and(|c| c.is_alphanumeric() || "_-".contains(c));
        out.push_str(if in_name { home } else { "~" });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Moves the last run's log to `8b.prev.log` and opens a fresh `8b.log` in `dir`.
fn rotate(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let current = dir.join(LOG_NAME);
    if current.exists() {
        fs::rename(&current, dir.join(PREV_NAME))?;
    }
    File::create(current)
}

/// A writer that passes `limit` bytes to `inner`, then one [`FULL_LINE`], then drops
/// everything.
struct Capped<W: Write> {
    inner: W,
    left: u64,
    full: bool,
}

impl<W: Write> Capped<W> {
    const fn new(inner: W, limit: u64) -> Self {
        Self { inner, left: limit, full: false }
    }
}

impl<W: Write> Write for Capped<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.full {
            return Ok(buf.len());
        }
        match u64::try_from(buf.len()) {
            Ok(len) if len <= self.left => {
                self.left -= len;
                self.inner.write_all(buf)?;
            }
            _ => {
                self.full = true;
                self.inner.write_all(FULL_LINE.as_bytes())?;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Sends each record to the file and to stderr; each sink keeps its own level.
struct Both {
    file: Option<Logger>,
    stderr: Logger,
}

impl Both {
    /// The most verbose level either sink wants.
    fn max_level(&self) -> LevelFilter {
        self.file.as_ref().map_or(LevelFilter::Off, Logger::filter).max(self.stderr.filter())
    }
}

impl Log for Both {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.stderr.enabled(metadata) || self.file.as_ref().is_some_and(|f| f.enabled(metadata))
    }

    fn log(&self, record: &Record<'_>) {
        self.stderr.log(record);
        if let Some(file) = &self.file {
            file.log(record);
        }
    }

    fn flush(&self) {
        self.stderr.flush();
        if let Some(file) = &self.file {
            file.flush();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn home_is_written_as_a_tilde() {
        assert_eq!(
            hide_home("saved /home/ada/8b/x and /home/ada", Some("/home/ada")),
            "saved ~/8b/x and ~"
        );
        assert_eq!(hide_home("/home/adam /home/ada.", Some("/home/ada")), "/home/adam ~.");
        assert_eq!(hide_home("/etc", Some("/")), "/etc");
        assert_eq!(hide_home("/home/ada", None), "/home/ada");
    }

    #[test]
    fn the_last_log_moves_to_prev() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(LOG_NAME), "old run").unwrap();
        let mut file = rotate(dir.path()).unwrap();
        file.write_all(b"new run").unwrap();
        assert_eq!(fs::read_to_string(dir.path().join(PREV_NAME)).unwrap(), "old run");
        assert_eq!(fs::read_to_string(dir.path().join(LOG_NAME)).unwrap(), "new run");
        // A second start replaces the old `.prev`.
        rotate(dir.path()).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join(PREV_NAME)).unwrap(), "new run");
    }

    #[test]
    fn a_missing_state_dir_is_made() {
        let dir = tempfile::tempdir().unwrap();
        assert!(rotate(&dir.path().join("a/b")).is_ok());
    }

    #[test]
    fn a_full_file_ends_with_one_log_full_line() {
        let mut out = Vec::new();
        let mut capped = Capped::new(&mut out, 11);
        capped.write_all(b"12345\n").unwrap();
        capped.write_all(b"6789\n").unwrap();
        capped.write_all(b"too much\n").unwrap();
        capped.write_all(b"dropped\n").unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), format!("12345\n6789\n{FULL_LINE}"));
    }

    #[test]
    fn the_build_kind_follows_the_sandbox_then_appimage() {
        assert_eq!(build_kind(true, true), "flatpak");
        assert_eq!(build_kind(false, true), "appimage");
        assert_eq!(build_kind(false, false), "native");
    }
}
