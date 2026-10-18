//! Canonical profile model.
//!
//! The fields every model shares are typed. The settings are one JSON object per group,
//! such as `sticks`, whose fields the model's description declares.
//! `schemas/profile-v1.schema.json` is the Pro 3's full shape.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::ids::Mode;
use super::macros::MacroDefinition;

/// The fields every profile has, by JSON name. A settings group may not take one.
pub const PROFILE_FIELDS: [&str; 10] = [
    "id",
    "name",
    "version",
    "kind",
    "device",
    "mode",
    "preferred_slot",
    "button_mappings",
    "macro_refs",
    "macros",
];

/// A full canonical profile (export/validation shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalProfile {
    /// Deterministic profile id.
    pub id: String,
    /// Human-readable name (1–16 chars).
    pub name: String,
    /// Schema version (always 1).
    pub version: u8,
    /// Schema kind discriminator.
    pub kind: String,
    /// Device discriminator, such as `"8bitdo-pro3"`.
    pub device: String,
    /// Operating mode.
    pub mode: Mode,
    /// Preferred slot, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_slot: Option<u8>,
    /// The settings groups, such as `sticks` and `vibration` on a Pro 3, by name. The
    /// JSON holds them beside the other fields. The description names each value by a
    /// JSON pointer, such as `/sticks/left_min_pct`.
    #[serde(flatten)]
    pub settings: Map<String, Value>,
    /// Button remaps.
    pub button_mappings: Vec<ButtonMapping>,
    /// Macro references (always empty for device readback).
    #[serde(default)]
    pub macro_refs: Vec<MacroRef>,
    /// The slot's macros, held in the file itself, each with its `macro_slot`. `None`
    /// when the file has no `macros` key, and always for a read from the pad. A file
    /// holds these or non-empty `macro_refs`, not both.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "embedded")]
    pub macros: Option<Vec<MacroDefinition>>,
}

impl CanonicalProfile {
    /// The settings value at JSON pointer `pointer`, such as `/vibration/left_level`.
    #[must_use]
    pub fn setting(&self, pointer: &str) -> Option<&Value> {
        let (group, rest) = split(pointer)?;
        let value = self.settings.get(group)?;
        if rest.is_empty() {
            Some(value)
        } else {
            value.pointer(rest)
        }
    }

    /// Replaces the settings value at `pointer` with `value`. Returns `false`, and
    /// changes nothing, when the profile has no value there.
    pub fn set_setting(&mut self, pointer: &str, value: Value) -> bool {
        let Some((group, rest)) = split(pointer) else { return false };
        let Some(old) = self.settings.get_mut(group) else { return false };
        let slot = if rest.is_empty() { Some(old) } else { old.pointer_mut(rest) };
        slot.map(|slot| *slot = value).is_some()
    }
}

/// Splits `/group/rest` into the group name and `/rest`, which is empty for `/group`.
fn split(pointer: &str) -> Option<(&str, &str)> {
    let path = pointer.strip_prefix('/')?;
    let group = path.split('/').next()?;
    Some((group, path.get(group.len()..)?))
}

/// The `macros` entries of profile JSON: `macro-v1` without `version` and `device`,
/// which the profile holds, plus `macro_slot`.
mod embedded {
    use serde::de::Error as _;
    use serde::{Deserialize as _, Deserializer, Serialize as _, Serializer};
    use serde_json::Value;

    use crate::model::macros::{macro_from_json, macro_to_json, MacroDefinition};

    /// The file keys of a `macro-v1` object that an entry leaves to its profile.
    const PROFILE_KEYS: [&str; 2] = ["version", "device"];

    /// Writes each macro as its entry.
    #[allow(clippy::ref_option)] // serde's `with` passes the field by reference
    pub(super) fn serialize<S: Serializer>(
        macros: &Option<Vec<MacroDefinition>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let entries: Option<Vec<Value>> = macros.as_ref().map(|defs| {
            defs.iter()
                .map(|def| {
                    let mut entry = macro_to_json(def, "");
                    if let Some(map) = entry.as_object_mut() {
                        for key in PROFILE_KEYS {
                            map.remove(key);
                        }
                        map.insert("macro_slot".to_owned(), def.macro_slot.into());
                    }
                    entry
                })
                .collect()
        });
        entries.serialize(serializer)
    }

    /// Reads each entry as a macro in its `macro_slot`.
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<MacroDefinition>>, D::Error> {
        Vec::<Value>::deserialize(deserializer)?
            .iter()
            .map(|entry| {
                let mut def = macro_from_json(entry).map_err(D::Error::custom)?;
                let slot = entry.get("macro_slot").and_then(Value::as_u64);
                let slot = slot.and_then(|n| u8::try_from(n).ok());
                def.macro_slot =
                    Some(slot.ok_or_else(|| D::Error::custom("a macro needs its macro_slot"))?);
                Ok(def)
            })
            .collect::<Result<_, _>>()
            .map(Some)
    }
}

/// A single button remap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonMapping {
    /// Source control name.
    pub source: String,
    /// Target control name, `"disabled"`, `"screenshot"` (Switch), a back-paddle
    /// output such as `"rp output"` (`DInput`), or the read-only `"unrecognised"`
    /// (keep the entry's bytes as read).
    pub target: String,
}

/// A reference from a profile to a macro file.
///
/// It reads the legacy single `trigger` of a v0.1.1 profile as a one-button
/// `trigger_buttons`, and refuses both fields together. It writes `trigger_buttons` only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "MacroRefFile")]
pub struct MacroRef {
    /// The buttons that start the macro, empty when none does.
    pub trigger_buttons: Vec<String>,
    /// Relative path to the macro JSON.
    pub path: String,
}

/// A [`MacroRef`] as a profile file may hold it.
#[derive(Deserialize)]
struct MacroRefFile {
    trigger: Option<String>,
    trigger_buttons: Option<Vec<String>>,
    path: String,
}

impl TryFrom<MacroRefFile> for MacroRef {
    type Error = String;

    fn try_from(file: MacroRefFile) -> Result<Self, String> {
        let trigger_buttons = match (file.trigger, file.trigger_buttons) {
            (None, Some(buttons)) => buttons,
            (Some(button), None) => vec![button],
            (Some(_), Some(_)) => {
                return Err("a macro ref has trigger or trigger_buttons, not both".into())
            }
            (None, None) => return Err("a macro ref needs trigger_buttons".into()),
        };
        Ok(Self { trigger_buttons, path: file.path })
    }
}

/// Raw profile payload from USB readback before mapping.
#[derive(Debug, Clone)]
pub struct RawProfilePayload {
    /// Raw 2348-byte profile blob.
    pub payload: Vec<u8>,
    /// 1-based slot index.
    pub source_slot: u8,
    /// 0-based profile index.
    pub source_profile_index: u8,
    /// Mode context for table selection.
    pub mode_hint: Mode,
}

/// A mapped profile plus provenance metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProfileSummary {
    /// Profile id.
    pub id: String,
    /// Profile name.
    pub name: String,
    /// Mode.
    pub mode: Mode,
    /// 1-based slot.
    pub source_slot: u8,
    /// 0-based profile index.
    pub source_profile_index: u8,
    /// Full canonical profile.
    pub canonical: CanonicalProfile,
}

/// Result of reading all on-device profiles.
#[derive(Debug, Clone, Default)]
pub struct ProfileReadResult {
    /// Mapped canonical profiles.
    pub profiles: Vec<CanonicalProfileSummary>,
    /// Raw blobs for diagnostics/dump.
    pub raw_blobs: Vec<Vec<u8>>,
    /// Firmware version the pad reports, such as `1.09`. Empty when the model has no
    /// way to read it.
    pub firmware_version: String,
}

/// Builds the canonical `{mode}-slot-{slot}-index-{index}` id.
#[must_use]
pub fn canonical_id(mode: Mode, source_slot: u8, source_profile_index: u8) -> String {
    format!("{}-slot-{source_slot}-index-{source_profile_index}", mode.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::pro3::{SWITCH, XINPUT};

    #[test]
    fn canonical_id_uses_mode_slot_index() {
        assert_eq!(canonical_id(XINPUT, 1, 0), "xinput-slot-1-index-0");
        assert_eq!(canonical_id(SWITCH, 2, 1), "switch-slot-2-index-1");
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn profile_json_round_trips() {
        let json = serde_json::json!({
            "id":"test","name":"Test","version":1,
            "kind":"8bitdo.pro3.profile","device":"8bitdo-pro3","mode":"xinput",
            "sticks":{"left_min_pct":0,"left_max_pct":100,"right_min_pct":0,"right_max_pct":100,
              "invert_left_x":false,"invert_left_y":false,"invert_right_x":false,"invert_right_y":false,
              "swap_sticks":false,"swap_dpad_with_left_stick":false},
            "triggers":{"left_min_pct":0,"left_max_pct":100,"right_min_pct":0,"right_max_pct":100,"swap_triggers":false},
            "vibration":{"left_level":3,"right_level":3},
            "button_mappings":[], "macro_refs":[]
        });
        let mut p: CanonicalProfile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(p.mode, XINPUT);
        let typed = json.as_object().unwrap().keys().filter(|k| !p.settings.contains_key(*k));
        assert!(typed.into_iter().all(|k| PROFILE_FIELDS.contains(&k.as_str())));
        assert_eq!(serde_json::to_value(&p).unwrap(), json);
        // The settings groups sit between `mode` and `button_mappings`, in name order.
        // The keys inside a group come out in name order too, not in the order the
        // description lists them: `serde_json` keeps a map sorted.
        let text = serde_json::to_string(&p).unwrap();
        let at = |key: &str| text.find(&format!("\"{key}\":")).unwrap();
        let order = ["mode", "sticks", "triggers", "vibration", "button_mappings", "macro_refs"];
        let positions: Vec<usize> = order.iter().map(|key| at(key)).collect();
        assert!(positions.is_sorted(), "{text}");
        let sticks: Vec<&String> =
            p.settings.get("sticks").and_then(Value::as_object).unwrap().keys().collect();
        assert!(sticks.is_sorted(), "{sticks:?}");
        assert!(at("left_max_pct") < at("left_min_pct"), "{text}");

        assert_eq!(p.setting("/vibration/left_level"), Some(&3.into()));
        assert!(p.set_setting("/vibration/left_level", 5.into()));
        assert_eq!(p.setting("/vibration/left_level"), Some(&5.into()));
        assert!(!p.set_setting("/vibration/missing", 1.into()));
        assert!(!p.set_setting("/lights/level", 1.into()), "no such group");
        assert_eq!(p.setting("vibration"), None, "a pointer starts with a slash");
    }
}
