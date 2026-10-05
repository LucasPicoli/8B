//! Open and save dialogs through the xdg desktop portal's `FileChooser`. Each call
//! blocks until the dialog closes, so it runs on a thread of its own.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// The portal answers 0 when the user picked a file, 1 when they cancelled.
const RESPONSE_OK: u32 = 0;
/// Numbers each request, so two open dialogs get different handle paths.
static REQUESTS: AtomicU32 = AtomicU32::new(0);
/// The filter both dialogs offer: name, then (glob kind, pattern).
const JSON_FILTER: (&str, &[(u32, &str)]) = ("Profile JSON", &[(0, "*.json")]);

#[allow(missing_docs, unreachable_pub)]
mod proxy {
    use std::collections::HashMap;

    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    #[zbus::proxy(
        interface = "org.freedesktop.portal.FileChooser",
        default_service = "org.freedesktop.portal.Desktop",
        default_path = "/org/freedesktop/portal/desktop",
        gen_async = false
    )]
    pub trait FileChooser {
        fn open_file(
            &self,
            parent_window: &str,
            title: &str,
            options: HashMap<&str, Value<'_>>,
        ) -> zbus::Result<OwnedObjectPath>;
        fn save_file(
            &self,
            parent_window: &str,
            title: &str,
            options: HashMap<&str, Value<'_>>,
        ) -> zbus::Result<OwnedObjectPath>;
    }

    #[zbus::proxy(
        interface = "org.freedesktop.portal.Request",
        default_service = "org.freedesktop.portal.Desktop",
        gen_async = false
    )]
    pub trait Request {
        #[zbus(signal)]
        fn response(&self, response: u32, results: HashMap<String, OwnedValue>)
            -> zbus::Result<()>;
    }
}

/// Which dialog to show.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Open,
    Save,
}

/// Asks for a profile file to open. `None` when the user cancelled.
///
/// # Errors
/// Returns why the portal could not show the dialog.
pub fn open_json(title: &str) -> Result<Option<PathBuf>, String> {
    choose(Kind::Open, title, None).map_err(|e| e.to_string())
}

/// Asks where to save a profile file, suggesting `name`. `None` when the user
/// cancelled.
///
/// # Errors
/// Returns why the portal could not show the dialog.
pub fn save_json(title: &str, name: &str) -> Result<Option<PathBuf>, String> {
    choose(Kind::Save, title, Some(name)).map_err(|e| e.to_string())
}

/// Shows the dialog and waits for its answer.
fn choose(kind: Kind, title: &str, name: Option<&str>) -> zbus::Result<Option<PathBuf>> {
    let connection = zbus::blocking::Connection::session()?;
    // Subscribe to the answer before asking, so a fast answer is not missed. The
    // portal builds the handle path from the caller's bus name and the token.
    let token = format!("eightb{}", REQUESTS.fetch_add(1, Ordering::Relaxed));
    let sender = connection.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_"));
    let handle =
        format!("/org/freedesktop/portal/desktop/request/{}/{token}", sender.unwrap_or_default());
    let request = proxy::RequestProxy::builder(&connection).path(handle)?.build()?;
    let mut answers = request.receive_response()?;

    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", token.as_str().into());
    options.insert("modal", true.into());
    options.insert("filters", vec![JSON_FILTER].into());
    if let Some(name) = name {
        options.insert("current_name", name.into());
    }
    let chooser = proxy::FileChooserProxy::new(&connection)?;
    let _: OwnedObjectPath = match kind {
        Kind::Open => chooser.open_file("", title, options)?,
        Kind::Save => chooser.save_file("", title, options)?,
    };

    let Some(answer) = answers.next() else { return Ok(None) };
    let args = answer.args()?;
    if args.response != RESPONSE_OK {
        return Ok(None);
    }
    Ok(args.results.get("uris").cloned().and_then(first_path))
}

/// The local path of the first `file://` URI in a portal `uris` answer.
fn first_path(uris: OwnedValue) -> Option<PathBuf> {
    let uris = <Vec<String>>::try_from(uris).ok()?;
    uris.first()?.strip_prefix("file://").map(|p| PathBuf::from(percent_decode(p)))
}

/// Undoes URI percent-encoding, such as `%20` for a space. A malformed escape is
/// kept as written.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        if let Some(decoded) =
            hex.and_then(|h| u8::from_str_radix(h, 16).ok()).filter(|_| b == b'%')
        {
            out.push(decoded);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_become_paths() {
        assert_eq!(percent_decode("/home/a%20b/%C3%A9.json"), "/home/a b/é.json");
        assert_eq!(percent_decode("/50%/x%2"), "/50%/x%2");
        let uris = |u: &[&str]| {
            OwnedValue::try_from(Value::from(u.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()))
                .ok()
        };
        assert_eq!(
            uris(&["file:///tmp/a%20b.json"]).and_then(first_path),
            Some(PathBuf::from("/tmp/a b.json"))
        );
        assert_eq!(uris(&["https://x/y.json"]).and_then(first_path), None);
        assert_eq!(uris(&[]).and_then(first_path), None);
    }
}
