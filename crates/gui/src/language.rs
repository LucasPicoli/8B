//! The interface language. Slint translates the `.slint` text from its bundled copy of
//! each `gui.po`, and `tr` translates the Rust-built text from the same file. [`select`]
//! moves both together.

use crate::render::render;
use crate::state::AppState;
use crate::ui::AppWindow;

include!(concat!(env!("OUT_DIR"), "/languages.rs"));

/// One bundled translation: a folder under `crates/gui/translations`.
pub struct Language {
    /// The folder name, a locale such as `pt_BR`.
    pub code: &'static str,
    /// The language in its own words, or the folder name when its `gui.po` has no
    /// `language name` entry.
    #[cfg_attr(not(test), expect(dead_code, reason = "the language menu shows it"))]
    pub name: &'static str,
    /// The folder's `gui.po`, compiled to a `.mo` for `tr`.
    mo: &'static [u8],
}

/// The bundled language that `locale` asks for: the whole locale first, then its
/// language part alone. `None` means English, the source language.
#[must_use]
pub fn matching(locale: &str) -> Option<&'static Language> {
    fn base(locale: &str) -> &str {
        locale.split(['-', '_', '@']).next().unwrap_or(locale)
    }
    let locale = locale.replace('-', "_");
    BUNDLED
        .iter()
        .find(|l| l.code == locale)
        .or_else(|| BUNDLED.iter().find(|l| base(l.code) == base(&locale)))
}

/// Shows the window in `language`, or in English for `None`, then renders `state` again
/// so the Rust-built text follows. Slint also takes the language's decimal separator.
///
/// # Errors
///
/// The language's `.mo` does not load, or Slint has no bundle for it. Nothing changes.
pub fn select(language: Option<&Language>, state: &AppState, ui: &AppWindow) -> Result<(), String> {
    if let Some(language) = language {
        let translator = tr::MoTranslator::from_vec_u8(language.mo.to_vec())
            .map_err(|e| format!("{}: {e}", language.code))?;
        slint::select_bundled_translation(language.code).map_err(|e| e.to_string())?;
        tr::set_translator!(translator);
    } else {
        slint::select_bundled_translation("").map_err(|e| e.to_string())?;
        tr::unset_translator!();
    }
    render(state, ui);
    Ok(())
}

/// Shows the window in the system language, or in English when none is bundled. Slint
/// took the system language for the `.slint` text already; this moves `tr` along.
pub fn follow_system(state: &AppState, ui: &AppWindow) {
    let locale = sys_locale::get_locale().unwrap_or_default();
    if let Err(e) = select(matching(&locale), state, ui) {
        log::warn!("staying in English: {e}");
        let _ = select(None, state, ui);
    }
}
