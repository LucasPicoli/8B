//! `dev` commands for reverse engineering a controller the app does not support yet.
//!
//! They print plain text, not JSON, and know no protocol: `list` shows every hidraw node,
//! `send` writes raw bytes and prints every report that comes back, and `diff` lists the
//! byte runs that differ between two dumps.

use std::path::Path;
use std::time::Duration;

use controller_core::devices::descriptions;
use controller_core::protocol::bytes::hex;
use controller_core::transport::raw::{exchange, list_hidraw, HidrawNode, SYSFS_HIDRAW};

/// Exit code when a node cannot be opened or a file cannot be read.
const EXIT_IO: i32 = 1;
/// Exit code for bad input, as clap uses it.
const EXIT_USAGE: i32 = 2;

/// Runs `dev list`: one line per hidraw node, and its report descriptor in hex when
/// `descriptors` is set.
pub fn run_list(descriptors: bool) -> i32 {
    let nodes = list_hidraw(Path::new(SYSFS_HIDRAW));
    if nodes.is_empty() {
        println!("no hidraw nodes under {SYSFS_HIDRAW}");
    }
    for node in &nodes {
        println!("{}", list_line(node));
        if descriptors {
            println!("    descriptor {}", hex(&node.report_descriptor));
        }
    }
    0
}

/// One `dev list` line: node, bus, ids, interface, descriptor size, name, and the
/// supported model whose config port it is, if any.
fn list_line(node: &HidrawNode) -> String {
    let interface = node.interface.map_or_else(|| "-".to_owned(), |i| i.to_string());
    let known = descriptions()
        .find_map(|d| {
            let port = d.config_ports.iter().find(|p| {
                (p.usb.vendor, p.usb.product) == (node.vendor, node.product)
                    && node.interface == Some(p.interface)
            })?;
            Some(format!("  [{} config port, {}]", d.short_name, port.mode))
        })
        .unwrap_or_default();
    format!(
        "{}  {} {:04x}:{:04x}  interface {interface}  descriptor {} bytes  {}{known}",
        node.node.display(),
        node.bus_name(),
        node.vendor,
        node.product,
        node.report_descriptor.len(),
        node.name,
    )
}

/// What `dev send` sends and which replies it prints.
pub struct Send<'a> {
    /// The hidraw node.
    pub node: &'a Path,
    /// The packet as hex arguments.
    pub bytes: &'a [String],
    /// Zero-pad the packet to this length.
    pub pad: Option<usize>,
    /// How long to collect reports, in milliseconds.
    pub wait_ms: u64,
    /// Print only reports that start with these hex bytes. `None` prints every report.
    pub matching: Option<&'a str>,
}

/// Runs `dev send`: writes the packet to the node, then prints every report read in the
/// wait that starts with the `matching` prefix.
pub fn run_send(send: &Send<'_>) -> i32 {
    let parsed = parse_hex(send.bytes).and_then(|p| padded(p, send.pad)).and_then(|packet| {
        let prefix = send.matching.map(|m| parse_hex(&[m.to_owned()])).transpose()?;
        Ok((packet, prefix.unwrap_or_default()))
    });
    let (packet, prefix) = match parsed {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("error: {e}");
            return EXIT_USAGE;
        }
    };
    println!("out         {}", hex(&packet));
    match exchange(send.node, &packet, Duration::from_millis(send.wait_ms)) {
        Ok(reports) => {
            let shown: Vec<_> = reports.iter().filter(|r| r.data.starts_with(&prefix)).collect();
            for report in &shown {
                println!("in  {:>5} ms {}", report.after.as_millis(), hex(&report.data));
            }
            println!("{} of {} reports in {} ms", shown.len(), reports.len(), send.wait_ms);
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            EXIT_IO
        }
    }
}

/// Parses hex bytes written as `81 04 00 01`, `81040001`, `0x81,0x04` or a mix.
fn parse_hex(args: &[String]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for token in args.iter().flat_map(|a| a.split(|c: char| c.is_whitespace() || c == ',')) {
        let digits = token.strip_prefix("0x").unwrap_or(token);
        if digits.len() % 2 != 0 {
            return Err(format!("'{token}' has an odd number of hex digits"));
        }
        for pair in digits.as_bytes().chunks(2) {
            let text = std::str::from_utf8(pair).map_err(|e| e.to_string())?;
            out.push(u8::from_str_radix(text, 16).map_err(|_| format!("'{token}' is not hex"))?);
        }
    }
    if out.is_empty() {
        return Err("no bytes to send".to_owned());
    }
    Ok(out)
}

/// Zero-pads `packet` to `pad` bytes. A packet already longer is an error.
fn padded(mut packet: Vec<u8>, pad: Option<usize>) -> Result<Vec<u8>, String> {
    let Some(len) = pad else { return Ok(packet) };
    if packet.len() > len {
        return Err(format!("{} bytes do not fit in --pad {len}", packet.len()));
    }
    packet.resize(len, 0);
    Ok(packet)
}

/// Runs `dev diff`: prints each run of bytes that differs between files `a` and `b`.
pub fn run_diff(a: &Path, b: &Path) -> i32 {
    let (old, new) = match (std::fs::read(a), std::fs::read(b)) {
        (Ok(old), Ok(new)) => (old, new),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("error: {e}");
            return EXIT_IO;
        }
    };
    if old.len() != new.len() {
        println!(
            "sizes differ: {} is {} bytes, {} is {}; comparing the first {}",
            a.display(),
            old.len(),
            b.display(),
            new.len(),
            old.len().min(new.len())
        );
    }
    let runs = changed_runs(&old, &new);
    for run in &runs {
        println!(
            "0x{:04x}  {:>3} bytes  {}  ->  {}",
            run.start,
            run.old.len(),
            hex(&run.old),
            hex(&run.new)
        );
    }
    println!("{} changed runs", runs.len());
    0
}

/// A run of consecutive bytes that differ between two dumps.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    /// Offset of the first changed byte.
    start: usize,
    /// The bytes in the first dump.
    old: Vec<u8>,
    /// The bytes in the second dump.
    new: Vec<u8>,
}

/// The changed runs over the bytes both dumps have.
fn changed_runs(old: &[u8], new: &[u8]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (offset, (&a, &b)) in old.iter().zip(new).enumerate() {
        if a == b {
            continue;
        }
        match runs.last_mut() {
            Some(run) if run.start + run.old.len() == offset => {
                run.old.push(a);
                run.new.push(b);
            }
            _ => runs.push(Run { start: offset, old: vec![a], new: vec![b] }),
        }
    }
    runs
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn hex_parses_every_spelling() {
        let want = vec![0x81, 0x04, 0x00, 0x01];
        assert_eq!(parse_hex(&args(&["81 04 00 01"])).unwrap(), want);
        assert_eq!(parse_hex(&args(&["81040001"])).unwrap(), want);
        assert_eq!(parse_hex(&args(&["0x81,0x04", "00", "01"])).unwrap(), want);
        assert!(parse_hex(&args(&["810"])).is_err());
        assert!(parse_hex(&args(&["zz"])).is_err());
        assert!(parse_hex(&args(&[" "])).is_err());
    }

    #[test]
    fn pad_fills_with_zeros_and_refuses_overflow() {
        assert_eq!(padded(vec![1, 2], Some(4)).unwrap(), [1, 2, 0, 0]);
        assert_eq!(padded(vec![1, 2], None).unwrap(), [1, 2]);
        assert!(padded(vec![1, 2, 3], Some(2)).is_err());
    }

    #[test]
    fn diff_groups_adjacent_changes() {
        let old = [0, 1, 2, 3, 4, 5, 6];
        let new = [0, 9, 9, 3, 4, 5, 7, 8];
        assert_eq!(
            changed_runs(&old, &new),
            [
                Run { start: 1, old: vec![1, 2], new: vec![9, 9] },
                Run { start: 6, old: vec![6], new: vec![7] },
            ]
        );
        assert_eq!(changed_runs(&old, &old), []);
    }
}
