//! The setting kinds of a settings frame beyond sliders and flags: a choice, a colour
//! and a ring of coloured segments.

use serde::Deserialize;

use crate::model::colour;

/// One of a few named values, shown as a drop-down list. The profile holds the id of
/// the picked option.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    /// Name shown to the user.
    pub label: String,
    /// JSON pointer into the profile, such as `/motion/map_to`.
    pub field: String,
    /// The values it may take, in display order.
    pub options: Vec<ChoiceOption>,
}

impl Choice {
    /// The option with id `id`.
    #[must_use]
    pub fn option(&self, id: &str) -> Option<&ChoiceOption> {
        self.options.iter().find(|o| o.id == id)
    }

    /// The ids of every option, in display order.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.options.iter().map(|o| o.id.clone()).collect()
    }
}

/// One value a [`Choice`] may take.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    /// What the profile holds, such as `right_stick`.
    pub id: String,
    /// Name shown to the user.
    pub label: String,
}

/// One colour. The profile holds it as `#rrggbb` (see [`colour`]).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colour {
    /// Name shown to the user.
    pub label: String,
    /// JSON pointer into the profile, such as `/lights/fire`.
    pub field: String,
}

/// A ring of lit segments with one colour each, drawn as a circle.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ring {
    /// Name shown to the user, such as "Left ring".
    pub label: String,
    /// The JSON pointer of each segment's colour, clockwise from the top.
    pub fields: Vec<String>,
}

/// Whether `value` is a colour a profile may hold.
#[must_use]
pub fn is_colour_value(value: Option<&serde_json::Value>) -> bool {
    value.and_then(serde_json::Value::as_str).and_then(colour::parse).is_some()
}
