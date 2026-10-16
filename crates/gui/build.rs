//! Compiles the markup. The fluent style supplies `Palette` (the desktop colour
//! scheme) and `AboutSlint`; the controls themselves are drawn in `ui/look/breeze`.
//!
//! Also lists the translations: each folder under `translations/` with a
//! `LC_MESSAGES/gui.po` is one language, so adding a language edits no code.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rspolib::prelude::AsBytes as _;

/// Where the `.po` files live, one folder per language.
const TRANSLATIONS: &str = "translations";

/// The pseudo-locale for the string coverage tests. It is never offered as a language.
const PSEUDO_LOCALE: &str = "qps";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Debug builds keep element ids, so the tests can walk the accessible tree.
    let debug = std::env::var("PROFILE").is_ok_and(|p| p == "debug");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".to_owned())
        .with_debug_info(debug)
        .with_bundled_translations(TRANSLATIONS)
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config)?;

    println!("cargo:rerun-if-changed={TRANSLATIONS}");
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(out.join("languages.rs"), languages(Path::new(TRANSLATIONS), &out)?)?;
    Ok(())
}

/// The `BUNDLED` table that `src/language.rs` includes, sorted by folder name. Each
/// `gui.po` is also compiled to `<code>.mo` in `out`: `tr` 0.1.11 drops the bytes it
/// is handed for a `.po`, but reads a `.mo`.
fn languages(root: &Path, out: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let folder = entry?.path();
        let po = folder.join("LC_MESSAGES/gui.po");
        let Some(code) = folder.file_name().and_then(|n| n.to_str()) else { continue };
        if code == PSEUDO_LOCALE || !po.is_file() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", po.display());
        let file = rspolib::pofile(po.to_str().ok_or("a translation path is not UTF-8")?)?;
        std::fs::write(out.join(format!("{code}.mo")), file.as_bytes())?;
        let name = native_name(&file).unwrap_or_else(|| code.to_owned());
        found.push((code.to_owned(), name));
    }
    found.sort();

    let mut table = String::from(
        "/// Every bundled translation. Written by `build.rs`.\npub const BUNDLED: &[Language] = &[\n",
    );
    for (code, name) in found {
        let mo = format!("concat!(env!(\"OUT_DIR\"), \"/{code}.mo\")");
        writeln!(
            table,
            "    Language {{ code: {code:?}, name: {name:?}, mo: include_bytes!({mo}) }},"
        )?;
    }
    table.push_str("];\n");
    Ok(table)
}

/// The language in its own words: the `msgstr` of `msgctxt "language name"`, `msgid "English"`.
/// `None` when the entry is missing or empty.
fn native_name(po: &rspolib::POFile) -> Option<String> {
    po.entries
        .iter()
        .find(|e| e.msgctxt.as_deref() == Some("language name") && e.msgid == "English")
        .and_then(|e| e.msgstr.clone())
        .filter(|name| !name.is_empty())
}
