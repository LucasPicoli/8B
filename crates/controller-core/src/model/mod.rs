//! Canonical data model (serde) and shared id/newtypes.
pub mod glide;
pub mod ids;
pub mod macros;
pub mod outcome;
pub mod profile;

pub use ids::{MacroSlot, Mode, Slot};
pub use macros::{GlideControl, MacroDefinition, MacroStep, REPEAT_CONTINUOUS};
pub use outcome::{DeviceReadiness, WriteResult};
pub use profile::{
    ButtonMapping, CanonicalProfile, CanonicalProfileSummary, MacroRef, ProfileReadResult,
    RawProfilePayload, PROFILE_FIELDS,
};
