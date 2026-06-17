//! Canonical data model (serde) and shared id/newtypes.
pub mod ids;
pub mod macros;
pub mod outcome;
pub mod profile;

pub use ids::{MacroSlot, Mode, Slot};
pub use macros::{MacroDefinition, MacroStep};
pub use outcome::DeviceReadiness;
pub use profile::{
    ButtonMapping, CanonicalProfile, CanonicalProfileSummary, MacroRef, ProfileReadResult,
    RawProfilePayload, Sticks, Triggers, TriggersAnalog, TriggersSwitch, Vibration,
};
