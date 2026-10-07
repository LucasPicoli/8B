//! The desktop accent colour, read from the xdg desktop portal and followed while
//! the window runs. With no portal, or no accent set, the theme keeps Breeze blue.

use std::thread;

use log::info;
use slint::{Color, ComponentHandle as _, Weak};
use zbus::zvariant::OwnedValue;

use crate::ui::{AppWindow, Theme};

/// The portal settings namespace that holds the accent.
const NAMESPACE: &str = "org.freedesktop.appearance";
/// The accent key: three sRGB doubles in 0..=1, out of range when unset.
const KEY: &str = "accent-color";
/// The contrast white text needs on a filled button (WCAG AA, normal text).
const MIN_CONTRAST: f32 = 4.5;
/// Each darkening step keeps this share of every channel.
const DARKEN_STEP: f32 = 0.95;

#[allow(missing_docs, unreachable_pub)]
mod proxy {
    use zbus::zvariant::{OwnedValue, Value};

    #[zbus::proxy(
        interface = "org.freedesktop.portal.Settings",
        default_service = "org.freedesktop.portal.Desktop",
        default_path = "/org/freedesktop/portal/desktop",
        gen_async = false
    )]
    pub trait Settings {
        fn read_one(&self, namespace: &str, key: &str) -> zbus::Result<OwnedValue>;
        #[zbus(signal)]
        fn setting_changed(&self, namespace: &str, key: &str, value: Value<'_>)
            -> zbus::Result<()>;
    }
}

/// Reads the accent now and on every change, on a thread of its own, and pushes
/// it into the window's theme.
pub fn follow_accent(ui: Weak<AppWindow>) {
    let spawned = thread::Builder::new().name("portal".to_owned()).spawn(move || {
        if let Err(e) = watch(&ui) {
            info!("no desktop accent: {e}");
        }
    });
    if let Err(e) = spawned {
        info!("no desktop accent: {e}");
    }
}

/// Pushes the accent at start, then each change, until the bus closes.
fn watch(ui: &Weak<AppWindow>) -> zbus::Result<()> {
    let connection = zbus::blocking::Connection::session()?;
    let settings = proxy::SettingsProxy::new(&connection)?;
    // Subscribe first, so a change between the read and the subscription is kept.
    let changes = settings.receive_setting_changed()?;
    push(ui, settings.read_one(NAMESPACE, KEY).ok().and_then(rgb));
    for change in changes {
        let args = change.args()?;
        if args.namespace == NAMESPACE && args.key == KEY {
            push(ui, args.value.try_to_owned().ok().and_then(rgb));
        }
    }
    Ok(())
}

/// The accent as 8-bit sRGB, or `None` when the portal reports it unset.
fn rgb(value: OwnedValue) -> Option<[u8; 3]> {
    let (r, g, b) = <(f64, f64, f64)>::try_from(value).ok()?;
    let channel = |c: f64| {
        // Rounded and inside 0..=255, so the cast is exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        (0.0..=1.0).contains(&c).then(|| (c * 255.0).round() as u8)
    };
    Some([channel(r)?, channel(g)?, channel(b)?])
}

/// Sets the theme's accent on the event loop: the desktop accent and its filled
/// variant, or Breeze blue for `None`.
fn push(ui: &Weak<AppWindow>, accent: Option<[u8; 3]>) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let theme = ui.global::<Theme<'_>>();
        if let Some(accent) = accent {
            let color = |[r, g, b]: [u8; 3]| Color::from_rgb_u8(r, g, b);
            theme.set_desktop_accent_color(color(accent));
            theme.set_desktop_accent_fill(color(fill_for_white(accent)));
        }
        theme.set_desktop_accent(accent.is_some());
    });
}

/// WCAG relative luminance of an sRGB colour.
fn luminance(rgb: [u8; 3]) -> f32 {
    let linear = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = rgb;
    0.2126_f32.mul_add(linear(r), 0.7152_f32.mul_add(linear(g), 0.0722 * linear(b)))
}

/// The contrast ratio of white text on `rgb`.
fn contrast_with_white(rgb: [u8; 3]) -> f32 {
    1.05 / (luminance(rgb) + 0.05)
}

/// `rgb` darkened in steps until white text on it reaches 4.5:1. A colour that
/// already reaches it comes back unchanged.
fn fill_for_white(rgb: [u8; 3]) -> [u8; 3] {
    // 0.95^255 scales any channel to 0, and black reaches 21:1.
    (0..=u8::MAX)
        .map(|step| {
            let scale = DARKEN_STEP.powi(i32::from(step));
            // Scaled down from a u8, so inside 0..=255.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            rgb.map(|c| (f32::from(c) * scale).round() as u8)
        })
        .find(|&fill| contrast_with_white(fill) >= MIN_CONTRAST)
        .unwrap_or([0, 0, 0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breeze_blue_darkens_until_white_text_reads() {
        let blue = [0x3d, 0xae, 0xe9];
        assert!(contrast_with_white(blue) < MIN_CONTRAST);
        let contrast = contrast_with_white(fill_for_white(blue));
        // One step changes the contrast by about a tenth, so it stops just past 4.5.
        assert!((MIN_CONTRAST..5.0).contains(&contrast), "{contrast}");
    }

    #[test]
    fn a_dark_enough_colour_is_kept() {
        assert_eq!(fill_for_white([0x1b, 0x74, 0xa8]), [0x1b, 0x74, 0xa8]);
        assert_eq!(fill_for_white([0, 0, 0]), [0, 0, 0]);
    }

    #[test]
    fn white_and_yellow_still_reach_the_contrast() {
        for rgb in [[255, 255, 255], [0xfd, 0xbc, 0x4b]] {
            assert!(contrast_with_white(fill_for_white(rgb)) >= MIN_CONTRAST);
        }
    }

    #[test]
    fn an_out_of_range_accent_is_unset() {
        let value = |r: f64| OwnedValue::try_from(zbus::zvariant::Value::from((r, 0.5, 1.0))).ok();
        assert_eq!(value(0.0).and_then(rgb), Some([0, 128, 255]));
        assert_eq!(value(-1.0).and_then(rgb), None);
    }
}
