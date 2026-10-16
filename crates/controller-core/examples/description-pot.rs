//! Prints the `.pot` entries for the text of every supported controller description.
//!
//! `cargo run -p controller-core --example description-pot`
//!
//! One entry per distinct string, with a `#. <model> <pointer>` comment for each place
//! it appears. A name, such as `XInput`, gets its own text as `msgstr`, so no language
//! has to translate it.

use controller_core::devices::models;

/// One `.pot` entry: the English text, where it appears, and whether it is a name.
struct Entry<'a> {
    /// The English text.
    text: &'a str,
    /// One `<model> <pointer>` line per place the text appears.
    places: Vec<String>,
    /// Some place gives it as a name, such as "PR" on its button.
    is_name: bool,
}

fn main() -> Result<(), controller_core::Error> {
    let mut entries: Vec<Entry<'_>> = Vec::new();
    for model in models() {
        let description = model.description()?;
        for prose in description.prose() {
            let place = format!("{} {}", description.short_name, prose.pointer);
            if let Some(entry) = entries.iter_mut().find(|e| e.text == prose.text) {
                entry.places.push(place);
                entry.is_name |= prose.is_name;
            } else {
                entries.push(Entry {
                    text: prose.text,
                    places: vec![place],
                    is_name: prose.is_name,
                });
            }
        }
    }
    for entry in entries {
        for place in &entry.places {
            println!("#. {place}");
        }
        let text = quoted(entry.text);
        println!("msgid {text}");
        println!("msgstr {}\n", if entry.is_name { text.as_str() } else { "\"\"" });
    }
    Ok(())
}

/// `text` as a `.po` string literal.
fn quoted(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    format!("\"{escaped}\"")
}
