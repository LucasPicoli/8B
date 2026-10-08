//! Concrete controller backends, and the registry of every supported model.
//!
//! A new model gets its own folder next to `pro3/` and one entry in [`MODELS`].

pub mod pro3;

use crate::description::ControllerDescription;
use crate::device::{ConfigPort, Model};

/// Every supported controller model.
static MODELS: [&dyn Model; 1] = [&pro3::Pro3];

/// Every supported controller model, in registry order.
#[must_use]
pub fn models() -> &'static [&'static dyn Model] {
    &MODELS
}

/// The description of every supported controller model.
pub fn descriptions() -> impl Iterator<Item = &'static ControllerDescription> {
    // A unit test loads every embedded description, so `ok()` drops nothing in a shipped build.
    models().iter().filter_map(|m| m.description().ok())
}

/// The model among `models` whose description lists `model_id`, the id a `START_CONFIG`
/// reply carries.
#[must_use]
pub fn find_model(models: &[&'static dyn Model], model_id: u16) -> Option<&'static dyn Model> {
    models.iter().copied().find(|m| m.description().is_ok_and(|d| d.model_ids.contains(&model_id)))
}

/// The config ports of every supported model. A USB id that two models share appears
/// once, with the port of the first model that lists it.
// ponytail: models that share a USB id must share its interface and framing; detection
// would have to try each candidate port before identifying the model otherwise.
#[must_use]
pub fn config_ports() -> Vec<ConfigPort> {
    let mut ports: Vec<ConfigPort> = Vec::new();
    for port in descriptions().flat_map(|d| d.config_ports.iter()) {
        if !ports.iter().any(|p| p.usb == port.usb) {
            ports.push(*port);
        }
    }
    ports
}
