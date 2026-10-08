//! Concrete controller backends, and the list of every supported model.
pub mod pro3;

use crate::description::ControllerDescription;

/// The description of every supported controller model. A new model adds its line here.
pub fn descriptions() -> impl Iterator<Item = &'static ControllerDescription> {
    // A unit test loads every embedded description, so `ok()` drops nothing in a shipped build.
    pro3::description().ok().into_iter()
}
