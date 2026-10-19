//! The text of a controller description that a person reads, for the translation
//! catalogue. Core stays English; the GUI translates each string where it shows it.

use super::{ControllerDescription, NumberField};

/// One string of a description, found where `description.json` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prose<'a> {
    /// The JSON pointer of the string in `description.json`, such as
    /// `/settings/0/frames/0/title`.
    pub pointer: String,
    /// The English text.
    pub text: &'a str,
    /// A name, such as `XInput` or `L4`: printed on the pad or the box, so every
    /// language keeps it as it is.
    pub is_name: bool,
}

impl ControllerDescription {
    /// Every string of this description that the app shows, prose and names, in
    /// document order. The catalogue build and the GUI's translated text share this
    /// one list of fields.
    #[must_use]
    pub fn prose(&self) -> Vec<Prose<'_>> {
        let mut out = Vec::new();
        out.push(Prose {
            pointer: "/display_name".into(),
            text: &self.display_name,
            is_name: true,
        });
        out.push(Prose { pointer: "/short_name".into(), text: &self.short_name, is_name: true });
        if let Some(family) = &self.family_name {
            out.push(Prose { pointer: "/family_name".into(), text: family, is_name: true });
        }
        for (m, mode) in self.modes.iter().enumerate() {
            out.push(Prose {
                pointer: format!("/modes/{m}/label"),
                text: &mode.label,
                is_name: true,
            });
            for (o, extra) in mode.extra_outputs.iter().enumerate() {
                // Mixed: "Screenshot" is prose, "PR" a name. A name maps to itself.
                let at = format!("/modes/{m}/extra_outputs/{o}");
                out.push(Prose {
                    pointer: format!("{at}/label"),
                    text: &extra.label,
                    is_name: false,
                });
                out.push(Prose {
                    pointer: format!("{at}/hint"),
                    text: &extra.hint,
                    is_name: false,
                });
                out.push(Prose {
                    pointer: format!("{at}/leaving_hint"),
                    text: &extra.leaving_hint,
                    is_name: false,
                });
            }
        }
        for (b, button) in self.buttons.iter().enumerate() {
            for (mode, label) in &button.labels {
                let pointer = format!("/buttons/{b}/labels/{mode}");
                out.push(Prose { pointer, text: label, is_name: true });
            }
            if let Some(printed) = &button.printed_label {
                out.push(Prose {
                    pointer: format!("/buttons/{b}/printed_label"),
                    text: printed,
                    is_name: true,
                });
            }
            for (mode, label) in &button.spoken_labels {
                let pointer = format!("/buttons/{b}/spoken_labels/{mode}");
                out.push(Prose { pointer, text: label, is_name: false });
            }
        }
        for (p, page) in self.settings.iter().enumerate() {
            let at = format!("/settings/{p}");
            out.push(Prose { pointer: format!("{at}/label"), text: &page.label, is_name: false });
            out.push(Prose { pointer: format!("{at}/lead"), text: &page.lead, is_name: false });
            for (f, frame) in page.frames.iter().enumerate() {
                let at = format!("{at}/frames/{f}");
                out.push(Prose {
                    pointer: format!("{at}/title"),
                    text: &frame.title,
                    is_name: false,
                });
                for (s, slider) in frame.sliders.iter().enumerate() {
                    let at = format!("{at}/sliders/{s}");
                    out.push(Prose {
                        pointer: format!("{at}/label"),
                        text: &slider.label,
                        is_name: false,
                    });
                    out.push(Prose {
                        pointer: format!("{at}/unit"),
                        text: &slider.unit,
                        is_name: true,
                    });
                    number_labels(&mut out, &format!("{at}/low"), &slider.low);
                    if let Some(high) = &slider.high {
                        number_labels(&mut out, &format!("{at}/high"), high);
                    }
                }
                for (g, flag) in frame.flags.iter().enumerate() {
                    let pointer = format!("{at}/flags/{g}/label");
                    out.push(Prose { pointer, text: &flag.label, is_name: false });
                }
            }
        }
        for (v, view) in self.views.iter().enumerate() {
            out.push(Prose {
                pointer: format!("/views/{v}/label"),
                text: &view.label,
                is_name: false,
            });
        }
        out.retain(|p| !p.text.is_empty());
        out
    }
}

/// Pushes the value names of `field`, found at `at`.
fn number_labels<'a>(out: &mut Vec<Prose<'a>>, at: &str, field: &'a NumberField) {
    for (i, label) in field.labels.iter().enumerate() {
        out.push(Prose { pointer: format!("{at}/labels/{i}"), text: label, is_name: false });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    /// The registered Ultimate.
    fn ultimate() -> &'static dyn crate::device::Model {
        crate::devices::model_named("Ultimate").unwrap()
    }
    use std::collections::BTreeSet;

    use serde_json::Value;

    use crate::device::ControllerSpec as _;
    use crate::devices::{pro3::Pro3, test_pad::TestPad};

    /// Keys whose string values, at any depth below, a person reads.
    const TEXT_KEYS: [&str; 12] = [
        "display_name",
        "family_name",
        "short_name",
        "label",
        "labels",
        "printed_label",
        "spoken_labels",
        "lead",
        "title",
        "unit",
        "hint",
        "leaving_hint",
    ];

    /// The pointer of every non-empty string under a [`TEXT_KEYS`] key in `json`.
    fn text_leaves(json: &Value, at: &str, text: bool, out: &mut BTreeSet<String>) {
        match json {
            Value::String(s) if text && !s.is_empty() => {
                out.insert(at.to_owned());
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    text_leaves(item, &format!("{at}/{i}"), text, out);
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    let text = text || TEXT_KEYS.contains(&key.as_str());
                    text_leaves(item, &format!("{at}/{key}"), text, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn every_text_field_is_listed_once_at_its_pointer() {
        let models = [
            (Pro3.description().unwrap(), include_str!("../../controllers/pro3/description.json")),
            (
                TestPad.description().unwrap(),
                include_str!("../../controllers/test-pad/description.json"),
            ),
            (
                ultimate().description().unwrap(),
                include_str!("../../controllers/ultimate-bluetooth/description.json"),
            ),
        ];
        for (description, raw) in models {
            let json: Value = serde_json::from_str(raw).unwrap();
            let prose = description.prose();
            for p in &prose {
                assert_eq!(
                    json.pointer(&p.pointer).and_then(Value::as_str),
                    Some(p.text),
                    "{}",
                    p.pointer
                );
            }
            let listed: BTreeSet<_> = prose.iter().map(|p| p.pointer.clone()).collect();
            assert_eq!(listed.len(), prose.len(), "a pointer listed twice");
            let mut expected = BTreeSet::new();
            text_leaves(&json, "", false, &mut expected);
            assert_eq!(listed, expected);
        }
        let pro3 = Pro3.description().unwrap().prose();
        let of = |text: &str| pro3.iter().find(|p| p.text == text).map(|p| p.is_name);
        assert_eq!(
            (of("XInput"), of("Dead zone"), of("Left stick")),
            (Some(true), Some(false), Some(false))
        );
    }
}
