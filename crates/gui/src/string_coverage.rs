//! The checks that every string a person reads goes through the translation files.
//!
//! - [`every_slint_string_is_marked`] parses each `.slint` file and fails on a literal
//!   outside `@tr`, and on a `Tooltip` whose text no screen reader gets.
//! - Under `GUI_TEST_LANG=qps`, [`walk`] reads the element tree after each shot and fails
//!   on a string that is not one `⟦…⟧` unit: Rust text without `tr!`, two translations
//!   glued into one string, or a `qps` catalogue older than the source. A unit may hold
//!   others, as `⟦Output for ⟦Turbo⟧⟧` does, and whole sentences may follow each other.
//!
//! A string passes as it is when it has no letters, a controller description marks it as
//! a name, or `translations/keep-english.txt` lists it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
// `NodeOrToken::into_token` lives in `rowan`, which this crate does not name.
#![allow(clippy::redundant_closure_for_method_calls)]

use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::ControlFlow;

use controller_core::device::ControllerSpec as _;
use controller_core::devices::{self, test_pad::TestPad};
use i_slint_backend_testing::{ElementHandle, ElementRoot};
use i_slint_compiler::parser::{SyntaxKind, SyntaxNode};

/// The pseudo-locale. Its `gui.po` wraps each `.slint` string in the markers below.
pub const PSEUDO: &str = "qps";
/// The marker before each translated string.
const OPEN: char = '⟦';
/// The marker after each translated string.
const CLOSE: char = '⟧';

/// The strings that stay English: one exact string per line of `keep-english.txt`,
/// then ` # ` and the reason. A line that starts with `#` is a comment.
fn kept() -> impl Iterator<Item = (&'static str, Option<&'static str>)> {
    include_str!("../translations/keep-english.txt")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split_once(" # ").map_or((l, None), |(text, why)| (text, Some(why))))
}

/// Every string that passes untranslated.
fn english() -> HashSet<&'static str> {
    let pads = devices::descriptions().chain(TestPad.description().ok());
    let names = pads.flat_map(|d| d.prose().into_iter().filter(|p| p.is_name).map(|p| p.text));
    kept().map(|(text, _)| text).chain(names).collect()
}

/// The profile and macro names the test fixtures type. They are a person's data, so the
/// walk reads them as it reads a number.
const TYPED: &[&str] = &[
    "A name far longer than sixteen",
    "A name far too long",
    "Edited",
    "Everything",
    "Hop",
    "Jump combo",
    "Mine too",
    "Mine",
    "Other pad",
    "Other",
    "Racing",
    "Ramp from Switch",
    "Ramp",
    "Slide jump",
    "Theirs",
    "Dash",
    "Edit 1",
    "Edit 2",
    "Edit 3",
];

/// The elements that show code, such as the command that installs the rule. Their value
/// passes as it is.
const CODE: &[&str] = &["CommandFrame::command", "FixDialog::command"];

/// `text` is one `⟦…⟧` unit, or whole sentences in units with one space or line break
/// between them.
/// What a unit holds is not read: it is the translation and its arguments.
fn translated(text: &str) -> bool {
    let (mut depth, mut units, mut outside) = (0_usize, Vec::new(), String::new());
    let mut unit = String::new();
    for c in text.chars() {
        match c {
            OPEN => depth += 1,
            CLOSE if depth == 0 => return false,
            CLOSE => {
                depth -= 1;
                if depth == 0 {
                    units.push(std::mem::take(&mut unit));
                    continue;
                }
            }
            _ if depth == 0 => {
                outside.push(c);
                continue;
            }
            _ => {}
        }
        if depth > 0 && !(c == OPEN && depth == 1) {
            unit.push(c);
        }
    }
    let joins = units.len().saturating_sub(1);
    let sentences = units.iter().take(joins).all(|u| u.ends_with(['.', '?', '!', ':']));
    depth == 0
        && !units.is_empty()
        && text.starts_with(OPEN)
        && text.ends_with(CLOSE)
        && outside.chars().count() == joins
        && outside.chars().all(|c| c == ' ' || c == '\n')
        && sentences
}

/// `text` needs no translation: no letters, a file path, or listed in [`english`].
fn stays(text: &str, english: &HashSet<&str>) -> bool {
    let path = text.starts_with('/') && !text.contains(' ');
    !text.chars().any(char::is_alphabetic) || path || english.contains(text)
}

/// The default names, which a slot or macro keeps as its name. Padded, "New profile"
/// would pass the 16 characters a name holds and lose its `⟧`.
const NAMES: &[&str] = &["New profile", "Macro {}"];

/// The `qps` text of `msgid`, as the `just translations` recipe writes it: a `~` per 2.5
/// characters in front, so the text runs about 40 % longer, as pt-BR does.
fn pseudo(msgid: &str) -> Cow<'static, str> {
    let long = !NAMES.contains(&msgid);
    let pad = "~".repeat(if long { (msgid.chars().count() * 2).div_ceil(5) } else { 0 });
    format!("{OPEN}{pad}{msgid}{CLOSE}").into()
}

/// Wraps every Rust msgid, as the `qps` catalogue wraps every `.slint` one. It also
/// covers description prose, which has no `qps` entry.
struct Pseudo;

impl tr::Translator for Pseudo {
    fn translate<'a>(&'a self, string: &'a str, _: Option<&'a str>) -> Cow<'a, str> {
        pseudo(string)
    }

    fn ntranslate<'a>(
        &'a self,
        n: u64,
        singular: &'a str,
        plural: &'a str,
        _: Option<&'a str>,
    ) -> Cow<'a, str> {
        pseudo(if n == 1 { singular } else { plural })
    }
}

/// Whether this run is the `GUI_TEST_LANG=qps` one.
pub fn pseudo_run() -> bool {
    std::env::var("GUI_TEST_LANG").is_ok_and(|l| l == PSEUDO)
}

/// In the `qps` run, wraps every Rust msgid from now on. `tr` keeps one translator per
/// process, so the first call installs it. A test state calls this before it builds any
/// text, so text the state keeps is wrapped too.
pub fn wrap_rust_text() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if pseudo_run() {
        ONCE.call_once(|| tr::set_translator!(Pseudo));
    }
}

/// Shows the `.slint` text from the `qps` catalogue and wraps every Rust msgid.
pub fn select_pseudo() {
    slint::select_bundled_translation(PSEUDO).unwrap();
    wrap_rust_text();
}

/// Fails on each string in `ui` a person reads that is not one `⟦…⟧` unit and does not
/// pass as English. `shot` names the screen in the message.
pub fn walk(shot: &str, ui: &impl ElementRoot) {
    let mut english = english();
    // The text inside a code field repeats the field's value, so the value is skipped there too.
    let mut code = HashSet::new();
    ui.root_element().visit_descendants(|e: ElementHandle| {
        if e.id().is_some_and(|id| CODE.contains(&id.as_str())) {
            code.extend(e.accessible_value());
        }
        ControlFlow::<()>::Continue(())
    });
    english.extend(code.iter().map(slint::SharedString::as_str));
    let mut bad = Vec::new();
    ui.root_element().visit_descendants(|e: ElementHandle| {
        let read = [
            e.accessible_label(),
            e.accessible_value(),
            e.accessible_placeholder_text(),
            e.accessible_description(),
        ];
        for text in read.into_iter().flatten() {
            let mut data = text.to_string();
            for name in TYPED {
                data = data.replace(name, "");
            }
            if !translated(&data) && !stays(&data, &english) && !bad.contains(&text) {
                bad.push(text);
            }
        }
        ControlFlow::<()>::Continue(())
    });
    assert!(
        bad.is_empty(),
        "{shot}: these strings skip the translation files. Wrap each in tr! or @tr as one \
         whole sentence, list a name in crates/gui/translations/keep-english.txt, or run \
         `just translations` if the qps catalogue is stale:\n{}",
        bad.iter().map(|t| format!("  {t:?}")).collect::<Vec<_>>().join("\n")
    );
}

/// Every `.slint` file under `ui`, sorted.
fn slint_files() -> Vec<std::path::PathBuf> {
    fn visit(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, out);
            } else if path.extension().is_some_and(|e| e == "slint") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui"), &mut out);
    out.sort();
    out
}

/// The line of `node` in its file, from 1.
fn line(source: &str, node: &SyntaxNode) -> usize {
    let at = usize::from(node.text_range().start());
    source[..at].lines().count().max(1)
}

/// The source text of the `name` binding directly in `element`, without its `;`.
fn binding(element: &SyntaxNode, name: &str) -> Option<String> {
    element.children().find_map(|b| {
        (b.kind() == SyntaxKind::Binding && b.child_text(SyntaxKind::Identifier)? == name)
            .then(|| b.child_node(SyntaxKind::BindingExpression))
            .flatten()
            .map(|e| e.text().to_string().trim().trim_end_matches(';').trim().to_owned())
    })
}

/// A `Tooltip` is in a screen reader's reach when it shows `@markdown("\{x}")` and `x` is
/// an element's `accessible-description`, or an element beside it binds
/// `accessible-description: x` or `text: x`. The walk reads that element, so it checks
/// the text too. `None` when the tooltip is in reach.
fn tooltip_out_of_reach(tooltip: &SyntaxNode) -> Option<String> {
    let text = binding(tooltip, "text")?;
    let shown = text.strip_prefix("@markdown(\"\\{").and_then(|t| t.strip_suffix("}\")"));
    let Some(shown) = shown else {
        return Some(format!("shows {text}, not @markdown(\"\\{{x}}\")"));
    };
    if shown.ends_with(".accessible-description") {
        return None;
    }
    let parent = tooltip.node.ancestors().skip(1).find(|n| n.kind() == SyntaxKind::Element)?;
    let parent = SyntaxNode { node: parent, source_file: tooltip.source_file.clone() };
    let mirrored = parent.descendants().filter(|n| n.kind() == SyntaxKind::Element).any(|e| {
        ["accessible-description", "text"]
            .iter()
            .any(|name| binding(&e, name).as_deref() == Some(shown) && e.node != tooltip.node)
    });
    (!mirrored).then(|| format!("no element beside it binds accessible-description: {shown}"))
}

/// A literal a person reads must sit in `@tr`. A literal passes in an import, an
/// `@image-url`, as a `@tr` context or plural, or when it stays English. Literals in
/// `@markdown` count.
#[test]
fn every_slint_string_is_marked() {
    use i_slint_compiler::diagnostics::BuildDiagnostics;
    use i_slint_compiler::literals::unescape_string;

    let english = english();
    let mut bad = Vec::new();
    for path in slint_files() {
        let source = std::fs::read_to_string(&path).unwrap();
        let doc = i_slint_compiler::parser::parse(
            source.clone(),
            Some(&path),
            &mut BuildDiagnostics::default(),
        );
        let file = path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap_or(&path).display();
        let at = |node: &SyntaxNode| format!("crates/gui/{file}:{}", line(&source, node));
        for token in doc.node.descendants_with_tokens().filter_map(|t| t.into_token()) {
            let Some(parent) = token.parent() else { continue };
            let allowed = matches!(
                parent.kind(),
                SyntaxKind::AtTr
                    | SyntaxKind::TrContext
                    | SyntaxKind::TrPlural
                    | SyntaxKind::ImportSpecifier
                    | SyntaxKind::AtImageUrl
            );
            // A `Path`'s `commands` is drawing data, never text.
            let drawing = parent.ancestors().any(|n| {
                n.kind() == SyntaxKind::Binding
                    && n.children_with_tokens()
                        .filter_map(|t| t.into_token())
                        .any(|t| t.kind() == SyntaxKind::Identifier && t.text() == "commands")
            });
            if token.kind() != SyntaxKind::StringLiteral || allowed || drawing {
                continue;
            }
            let text = unescape_string(token.text()).unwrap_or_default();
            if !stays(&text, &english) {
                let parent = SyntaxNode { node: parent, source_file: doc.source_file.clone() };
                bad.push(format!("{}: {text:?} is not in @tr", at(&parent)));
            }
        }
        for element in doc.descendants().filter(|n| n.kind() == SyntaxKind::Element) {
            let tooltip = element.child_node(SyntaxKind::QualifiedName);
            if tooltip.is_some_and(|q| q.text().to_string().trim() == "Tooltip") {
                if let Some(why) = tooltip_out_of_reach(&element) {
                    bad.push(format!("{}: Tooltip {why}", at(&element)));
                }
            }
        }
    }
    for (text, why) in kept() {
        if why.is_none_or(|w| w.trim().is_empty()) {
            bad.push(format!("keep-english.txt: {text:?} gives no ` # ` reason"));
        }
    }
    assert!(
        bad.is_empty(),
        "wrap each string in @tr(...), or list a name in crates/gui/translations/keep-english.txt:\n{}",
        bad.join("\n")
    );
}

#[test]
fn a_string_passes_as_one_unit_or_whole_sentences() {
    for ok in ["⟦Add step⟧", "⟦Output for ⟦Turbo⟧⟧", "⟦Unplugged.⟧ ⟦Your edits are kept.⟧"]
    {
        assert!(translated(ok), "{ok}");
    }
    for glued in
        ["Add step", "⟦Left stick⟧, ⟦Glide⟧", "⟦A⟧ ⟦B⟧", "⟦Read failed.⟧ timed out", "⟦A⟧⟧", "⟦A"]
    {
        assert!(!translated(glued), "{glued}");
    }
    let english = english();
    assert!(stays("+3 %", &english) && stays("XInput", &english) && !stays("Dead zone", &english));
    assert!(stays("/etc/udev/rules.d/70-8b.rules", &english) && !stays("/ is a path", &english));
}
