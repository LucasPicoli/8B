//! The interface language. Slint translates the `.slint` text from its bundled copy of
//! each `gui.po`, and `tr` translates the Rust-built text from the same file. [`select`]
//! moves both together. The ☰ menu picks the language, and `settings.json` keeps the pick.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use slint::{ComponentHandle as _, VecModel};

use crate::preferences;
use crate::render::render;
use crate::state::AppState;
use crate::ui::{AppWindow, LanguageChoice, Languages};

include!(concat!(env!("OUT_DIR"), "/languages.rs"));

/// One bundled translation: a folder under `crates/gui/translations`.
pub struct Language {
    /// The folder name, a locale such as `pt_BR`.
    pub code: &'static str,
    /// The language in its own words, or the folder name when its `gui.po` has no
    /// `language name` entry.
    pub name: &'static str,
    /// The folder's `gui.po`, compiled to a `.mo` for `tr`.
    mo: &'static [u8],
}

/// The code that picks English, the source language, which has no folder.
const ENGLISH: &str = "en";

/// The language a saved `code` names: `None` for English.
///
/// # Errors
///
/// No bundled folder carries `code`, for example after an update removed it.
pub fn by_code(code: &str) -> Result<Option<&'static Language>, String> {
    if code == ENGLISH {
        return Ok(None);
    }
    BUNDLED.iter().find(|l| l.code == code).map(Some).ok_or_else(|| format!("no language {code}"))
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

/// `text` from a controller description, such as a frame title, in the current language.
/// English when the catalogue has no entry for it. Code that compares description text
/// keeps the English and translates only what it shows.
#[must_use]
pub fn prose(text: &str) -> String {
    // A variable, not a literal: `xtr` skips it, and `description-pot` lists the text.
    tr::tr!(text)
}

/// The ☰ menu's rows after "System default": English, then each bundled language.
fn choices() -> impl Iterator<Item = (&'static str, &'static str)> {
    std::iter::once((ENGLISH, "English")).chain(BUNDLED.iter().map(|l| (l.code, l.name)))
}

/// Fills the ☰ menu's language list.
pub fn fill_menu(ui: &AppWindow) {
    let rows: Vec<_> = choices()
        .map(|(code, name)| LanguageChoice { code: code.into(), name: name.into() })
        .collect();
    ui.global::<Languages<'_>>().set_choices(Rc::new(VecModel::from(rows)).into());
}

/// Fills the ☰ menu, then shows the window in the language saved in `settings`, else in
/// the system language. A saved language that is gone logs one line and counts as the
/// system default.
pub fn start(settings: &Path, state: &AppState, ui: &AppWindow) {
    fill_menu(ui);
    let menu = ui.global::<Languages<'_>>();
    let saved = preferences::saved_language(settings).unwrap_or_default();
    if let Err(e) = apply(&saved, state, ui) {
        log::warn!("{e}: following the system language");
        follow_system(state, ui);
        menu.set_current(0);
        return;
    }
    menu.set_current(row_of(&saved));
}

/// Starts in the language saved in `settings.json`, then switches and saves on each pick
/// from the ☰ menu.
pub fn wire(state: &Rc<RefCell<AppState>>, ui: &AppWindow) {
    let settings =
        preferences::settings_path(std::env::var_os("XDG_CONFIG_HOME"), std::env::var_os("HOME"));
    start(&settings, &state.borrow(), ui);
    let (state, weak) = (Rc::clone(state), ui.as_weak());
    ui.global::<Languages<'_>>().on_picked(move |row| {
        if let Some(ui) = weak.upgrade() {
            pick(row, &settings, &state.borrow(), &ui);
        }
    });
}

/// The ☰ menu's pick: `row` 0 for the system default, then one per choice. Switches at
/// once and saves the pick in `settings`. A pick that fails changes nothing and logs one
/// line.
pub fn pick(row: i32, settings: &Path, state: &AppState, ui: &AppWindow) {
    let code = usize::try_from(row)
        .ok()
        .and_then(|r| r.checked_sub(1))
        .and_then(|r| choices().nth(r))
        .map_or("", |(code, _)| code);
    if let Err(e) = apply(code, state, ui) {
        log::warn!("cannot switch the language: {e}");
        return;
    }
    ui.global::<Languages<'_>>().set_current(row_of(code));
    if let Err(e) = preferences::save_language(settings, Some(code).filter(|c| !c.is_empty())) {
        log::warn!("cannot save the language in {}: {e}", settings.display());
    }
}

/// The ☰ menu row of `code`: 0 for the system default or an unknown code.
fn row_of(code: &str) -> i32 {
    choices().position(|(c, _)| c == code).and_then(|i| i32::try_from(i + 1).ok()).unwrap_or(0)
}

/// Shows the window in `code`, or in the system language for an empty code.
fn apply(code: &str, state: &AppState, ui: &AppWindow) -> Result<(), String> {
    if code.is_empty() {
        follow_system(state, ui);
        return Ok(());
    }
    select(by_code(code)?, state, ui)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_code_names_english_a_folder_or_nothing() {
        assert!(matches!(by_code("en"), Ok(None)));
        assert!(matches!(by_code("pt_BR"), Ok(Some(l)) if l.code == "pt_BR"));
        // A language whose folder is gone.
        assert!(by_code("xx_XX").is_err());
        assert_eq!(row_of(""), 0);
        assert_eq!(row_of("en"), 1);
        assert_eq!(row_of("pt_BR"), 2);
    }
}
