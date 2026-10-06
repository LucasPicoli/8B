//! Finds the programs that hold the controller's config node open. Another program
//! reading the node can steal the replies to a read, so a failed read names them.

use std::path::Path;

/// Where the kernel lists processes.
pub const PROC: &str = "/proc";

/// The names of the processes under `proc` with `node` open, sorted, each once.
/// Process `own` (8B itself) is left out. Only the user's own processes can be
/// read, so other users' processes are left out too.
#[must_use]
pub fn holders(proc: &Path, node: &Path, own: u32) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(proc) else { return vec![] };
    let mut names: Vec<String> = dir
        .flatten()
        .filter(|p| {
            p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()).is_some_and(|pid| pid != own)
        })
        .filter(|p| {
            std::fs::read_dir(p.path().join("fd")).is_ok_and(|fds| {
                fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t == node))
            })
        })
        .filter_map(|p| std::fs::read_to_string(p.path().join("comm")).ok())
        .map(|comm| comm.trim().to_owned())
        .filter(|comm| !comm.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The sentence that names `holders` and asks the user to close them, or `None`
/// when there are none.
#[must_use]
pub fn sentence(holders: &[String]) -> Option<String> {
    let (last, rest) = holders.split_last()?;
    if rest.is_empty() {
        return Some(format!("{last} also has the controller open. Close it and try again."));
    }
    Some(format!(
        "{} and {last} also have the controller open. Close them and try again.",
        rest.join(", ")
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;

    /// Adds process `pid` named `comm` to a fake `/proc`, with `fds` open.
    fn process(proc: &Path, pid: &str, comm: &str, fds: &[&str]) {
        let fd = proc.join(pid).join("fd");
        fs::create_dir_all(&fd).unwrap();
        fs::write(proc.join(pid).join("comm"), format!("{comm}\n")).unwrap();
        for (i, target) in fds.iter().enumerate() {
            symlink(target, fd.join(i.to_string())).unwrap();
        }
    }

    #[test]
    fn names_each_holder_once_and_leaves_out_8b() {
        let proc = tempfile::tempdir().unwrap();
        let p = proc.path();
        process(p, "100", "steam", &["/dev/null", "/dev/hidraw7"]);
        process(p, "101", "steam", &["/dev/hidraw7"]);
        process(p, "200", "winedevice.exe", &["/dev/hidraw7"]);
        process(p, "300", "firefox", &["/dev/hidraw4"]);
        process(p, "400", "8b", &["/dev/hidraw7"]);
        fs::create_dir_all(p.join("self")).unwrap();
        fs::write(p.join("uptime"), "1 1\n").unwrap();
        // Another user's process: no fd directory to read.
        fs::create_dir_all(p.join("500")).unwrap();
        fs::write(p.join("500/comm"), "root-thing\n").unwrap();
        let found = holders(p, Path::new("/dev/hidraw7"), 400);
        assert_eq!(found, ["steam", "winedevice.exe"]);
    }

    #[test]
    fn no_proc_means_no_holders() {
        assert_eq!(
            holders(Path::new("/nonexistent"), Path::new("/dev/hidraw7"), 1),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_sentence_agrees_with_the_count() {
        let names = |n: &[&str]| n.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        assert_eq!(sentence(&[]), None);
        assert_eq!(
            sentence(&names(&["steam"])).unwrap(),
            "steam also has the controller open. Close it and try again."
        );
        assert_eq!(
            sentence(&names(&["steam", "winedevice.exe"])).unwrap(),
            "steam and winedevice.exe also have the controller open. Close them and try again."
        );
        assert_eq!(
            sentence(&names(&["a", "b", "c"])).unwrap(),
            "a, b and c also have the controller open. Close them and try again."
        );
    }
}
