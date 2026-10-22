//! The colour settings of a tab, and its rings of coloured segments, such as the
//! Ultimate 2's light rings. The profile holds each colour as `#rrggbb`.
//!
//! A ring is drawn as `n` arcs in a 100 by 100 box, clockwise from the top. The user
//! selects segments, then one editor sets the colour of every selected one.

use std::collections::BTreeSet;
use std::rc::Rc;

use controller_core::description::{self as described, ControllerDescription};
use controller_core::model::{colour, Mode};
use serde_json::Value;
use slint::{Color, ModelRc, VecModel};
use tr::tr;

use crate::language::prose;
use crate::settings::patch;
use crate::state::AppState;
use crate::ui::{ColourRow, RingEditor, RingRow, RingSegment};

/// The side of the box a ring is drawn in.
const BOX: f64 = 100.0;
/// The outer radius of a ring, in box units.
const OUTER: f64 = 49.0;
/// The inner radius of a ring, in box units.
const INNER: f64 = 34.0;
/// The gap on each side of a segment, in degrees, so neighbours stay apart.
const GAP: f64 = 1.5;
/// How far outside the ring a click still picks a segment, in box units.
const SLACK: f64 = 4.0;

/// Which ring segments the colour editor sets, by JSON pointer. Kept across tabs and
/// slots: a pointer the slot's tabs lack selects nothing.
#[derive(Debug, Default)]
pub struct Segments {
    /// The selected segments.
    pub selected: BTreeSet<String>,
    /// Segment n of each ring of a frame goes with segment n of the other.
    pub mirror: bool,
    /// The segment a Shift click selects from.
    pub anchor: Option<String>,
}

/// The point at `radius` and `degrees` clockwise from the top of the box.
fn point(radius: f64, degrees: f64) -> (f64, f64) {
    let rad = (degrees - 90.0).to_radians();
    let centre = BOX / 2.0;
    (radius.mul_add(rad.cos(), centre), radius.mul_add(rad.sin(), centre))
}

/// The outline of segment `i` of `n`, as SVG path commands in the box.
#[must_use]
pub fn segment_path(i: usize, n: usize) -> String {
    // A ring has a few dozen segments at most.
    #[allow(clippy::cast_precision_loss)]
    let (span, i) = (360.0 / n.max(1) as f64, i as f64);
    let (start, end) = (i.mul_add(span, GAP), (i + 1.0).mul_add(span, -GAP));
    let large = u8::from(end - start > 180.0);
    let (o1, o2) = (point(OUTER, start), point(OUTER, end));
    let (i1, i2) = (point(INNER, start), point(INNER, end));
    format!(
        "M {:.2} {:.2} A {OUTER} {OUTER} 0 {large} 1 {:.2} {:.2} L {:.2} {:.2} \
         A {INNER} {INNER} 0 {large} 0 {:.2} {:.2} Z",
        o1.0, o1.1, o2.0, o2.1, i2.0, i2.1, i1.0, i1.1
    )
}

/// The segment of a ring of `n` under the point `(x, y)` of the box, if any.
#[must_use]
pub fn segment_at(n: usize, x: f64, y: f64) -> Option<usize> {
    let (dx, dy) = (x - BOX / 2.0, y - BOX / 2.0);
    let radius = dx.hypot(dy);
    if n == 0 || radius < INNER - SLACK || radius > OUTER + SLACK {
        return None;
    }
    let degrees = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
    // In 0 to 360 over a positive span: the floor is a small whole number.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = (degrees / (360.0 / n as f64)).floor() as usize;
    Some(index.min(n - 1))
}

/// Where `field` sits among the rings of `mode`: its frame, its ring and its index.
fn locate<'a>(
    d: &'a ControllerDescription,
    mode: Mode,
    field: &str,
) -> Option<(&'a described::SettingsFrame, usize, usize)> {
    d.pages(mode).flat_map(|p| &p.frames).find_map(|frame| {
        frame.rings.iter().enumerate().find_map(|(r, ring)| {
            let i = ring.fields.iter().position(|f| f == field)?;
            Some((frame, r, i))
        })
    })
}

/// Whether the mirror applies to `frame`: two rings of the same size.
fn mirrored(frame: &described::SettingsFrame) -> bool {
    matches!(frame.rings.as_slice(), [a, b] if a.fields.len() == b.fields.len())
}

/// `field`, and with the mirror on its partner in the other ring of its frame.
fn with_partner(d: &ControllerDescription, mode: Mode, field: &str, mirror: bool) -> Vec<String> {
    let mut out = vec![field.to_owned()];
    if let Some((frame, r, i)) = locate(d, mode, field).filter(|(f, ..)| mirror && mirrored(f)) {
        if let Some(other) = frame.rings.get(1 - r).and_then(|ring| ring.fields.get(i)) {
            out.push(other.clone());
        }
    }
    out
}

/// The colour code `json` holds at `pointer`.
fn code(json: &Value, pointer: &str) -> Option<u32> {
    json.pointer(pointer).and_then(Value::as_str).and_then(colour::parse)
}

/// The red, green and blue bytes of `code`.
const fn channels(code: u32) -> [u8; 3] {
    let [_, r, g, b] = code.to_be_bytes();
    [r, g, b]
}

/// `code` with `channel` (0 red, 1 green, 2 blue) set to `value`, rounded and kept in
/// 0 to 255. Any other channel leaves it as it is.
fn with_channel(code: u32, channel: usize, value: f32) -> u32 {
    let mut rgb = channels(code);
    if let Some(c) = rgb.get_mut(channel) {
        // In 0 to 255 after the clamp.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = value.round().clamp(0.0, 255.0) as u8;
        *c = v;
    }
    let [r, g, b] = rgb;
    u32::from_be_bytes([0, r, g, b])
}

/// `code` as the user reads it, such as `#7E3200`.
fn hex(code: u32) -> String {
    colour::format(code).to_uppercase()
}

/// The colour a hex field holds, with or without its `#`.
fn typed(text: &str) -> Option<u32> {
    let text = text.trim();
    colour::parse(text).or_else(|| colour::parse(&format!("#{text}")))
}

/// Whether `text` is a colour the hex field takes.
#[must_use]
pub fn is_typed_colour(text: &str) -> bool {
    typed(text).is_some()
}

/// A colour row named `label`, for `field`, showing `code` (none for a mix) and `was`.
fn colour_row(label: String, field: &str, code: Option<u32>, first: u32, was: String) -> ColourRow {
    let [r, g, b] = channels(code.unwrap_or(first));
    ColourRow {
        label: label.into(),
        field: field.into(),
        hex: code.map(hex).unwrap_or_default().into(),
        red: f32::from(r),
        green: f32::from(g),
        blue: f32::from(b),
        swatch: Color::from_rgb_u8(r, g, b),
        was: was.into(),
    }
}

/// The rows of `frame`'s colours, filled from `shown`, with what differs from `pad`. A
/// colour that `shown` lacks is left out.
pub fn rows(
    frame: &described::SettingsFrame,
    shown: &Value,
    pad: Option<&Value>,
) -> Vec<ColourRow> {
    frame
        .colours
        .iter()
        .filter_map(|c| {
            let now = code(shown, &c.field)?;
            let was = pad.and_then(|p| code(p, &c.field)).filter(|&old| old != now).map(hex);
            Some(colour_row(prose(&c.label), &c.field, Some(now), now, was.unwrap_or_default()))
        })
        .collect()
}

/// The rings of `frame`, filled from `shown`, with what differs from `pad` and what
/// `segments` selects. A ring that `shown` lacks a segment of is left out.
pub fn rings(
    frame: &described::SettingsFrame,
    shown: &Value,
    pad: Option<&Value>,
    segments: &Segments,
) -> Vec<RingRow> {
    frame
        .rings
        .iter()
        .filter_map(|ring| {
            let n = ring.fields.len();
            let label = prose(&ring.label);
            let cells = ring.fields.iter().enumerate().map(|(i, field)| {
                let now = code(shown, field)?;
                let [r, g, b] = channels(now);
                let changed = pad.and_then(|p| code(p, field)).is_some_and(|old| old != now);
                let spoken = tr!(
                    "{ring}, segment {n}, {colour}",
                    ring = label,
                    n = i + 1,
                    colour = hex(now)
                );
                Some(RingSegment {
                    field: field.as_str().into(),
                    path: segment_path(i, n).into(),
                    fill: Color::from_rgb_u8(r, g, b),
                    selected: segments.selected.contains(field),
                    changed,
                    spoken: spoken.into(),
                })
            });
            let cells: Vec<RingSegment> = cells.collect::<Option<_>>()?;
            let changed = cells.iter().filter(|c| c.changed).count();
            Some(RingRow {
                label: label.into(),
                changed: i32::try_from(changed).unwrap_or(i32::MAX),
                segments: ModelRc::from(Rc::new(VecModel::from(cells))),
            })
        })
        .collect()
}

/// The editor of `frame`'s selected segments, or none when it has no rings. Its hex is
/// empty when they hold different colours, and its sliders show the first of them.
pub fn editors(
    frame: &described::SettingsFrame,
    shown: &Value,
    segments: &Segments,
) -> Vec<RingEditor> {
    if frame.rings.is_empty() {
        return Vec::new();
    }
    let picked: Vec<u32> = frame
        .rings
        .iter()
        .flat_map(|r| &r.fields)
        .filter(|f| segments.selected.contains(*f))
        .filter_map(|f| code(shown, f))
        .collect();
    let first = picked.first().copied().unwrap_or_default();
    let same = picked.iter().all(|&c| c == first).then_some(first).filter(|_| !picked.is_empty());
    vec![RingEditor {
        count: i32::try_from(picked.len()).unwrap_or(i32::MAX),
        colour: colour_row(tr!("Selected segments"), "", same, first, String::new()),
        mirror: segments.mirror,
        offer_mirror: mirrored(frame),
    }]
}

impl AppState {
    /// Sets the colour at `pointer` in the selected slot to `text`, `#rrggbb` with or
    /// without its `#`. Text that is no colour, or a pointer that names none, changes
    /// nothing.
    pub fn set_colour(&mut self, pointer: &str, text: &str) {
        if let Some(code) = typed(text) {
            self.set_colours(&[pointer.to_owned()], |_| code);
        }
    }

    /// Sets one channel (0 red, 1 green, 2 blue) of the colour at `pointer`.
    pub fn set_colour_channel(&mut self, pointer: &str, channel: usize, value: f32) {
        self.set_colours(&[pointer.to_owned()], |old| with_channel(old, channel, value));
    }

    /// Sets every selected segment to `text`, as [`Self::set_colour`] does.
    pub fn set_selection_colour(&mut self, text: &str) {
        if let Some(code) = typed(text) {
            let selected = self.selected_segments();
            self.set_colours(&selected, |_| code);
        }
    }

    /// Sets every selected segment to the first one's colour with `channel` changed.
    pub fn set_selection_channel(&mut self, channel: usize, value: f32) {
        let selected = self.selected_segments();
        let shown = self.shown_json();
        let first = selected.iter().find_map(|f| shown.as_ref().and_then(|j| code(j, f)));
        if let Some(first) = first {
            let new = with_channel(first, channel, value);
            self.set_colours(&selected, |_| new);
        }
    }

    /// The selected segments the selected slot shows, in ring order.
    fn selected_segments(&self) -> Vec<String> {
        let Some((mode, _)) = self.selected_slot() else { return Vec::new() };
        let rings = self.description().pages(mode).flat_map(|p| &p.frames).flat_map(|f| &f.rings);
        rings
            .flat_map(|r| &r.fields)
            .filter(|f| self.segments.selected.contains(*f))
            .cloned()
            .collect()
    }

    /// The selected slot's profile on screen, as JSON.
    fn shown_json(&self) -> Option<Value> {
        let (mode, number) = self.selected_slot()?;
        serde_json::to_value(self.slot(mode, number).shown()?).ok()
    }

    /// Sets each colour of `pointers` the slot's tabs show to `new` of its old code.
    fn set_colours(&mut self, pointers: &[String], new: impl Fn(u32) -> u32) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let description = self.description();
        let pointers: Vec<&String> =
            pointers.iter().filter(|p| description.is_colour(mode, p)).collect();
        if pointers.is_empty() {
            return;
        }
        self.edit(|p| {
            for pointer in pointers {
                if let Some(old) =
                    p.setting(pointer).and_then(Value::as_str).and_then(colour::parse)
                {
                    patch(p, pointer, colour::format(new(old)).into());
                }
            }
        });
    }

    /// Selects segment `field`: alone, added to or taken off the selection (`add`, as
    /// a Ctrl click), or with every segment from the anchor in its ring (`range`, as a
    /// Shift click). With the mirror on, its partner goes with it.
    pub fn click_segment(&mut self, field: &str, add: bool, range: bool) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let d = self.description();
        let Some((frame, r, i)) = locate(d, mode, field) else { return };
        let mirror = self.segments.mirror;
        let anchor = self.segments.anchor.as_deref().and_then(|a| locate(d, mode, a));
        let run = match anchor {
            Some((_, ar, ai)) if range && ar == r => {
                let fields =
                    frame.rings.get(r).map(|ring| ring.fields.as_slice()).unwrap_or_default();
                fields.get(ai.min(i)..=ai.max(i)).unwrap_or_default().to_vec()
            }
            _ => vec![field.to_owned()],
        };
        let picked: Vec<String> =
            run.iter().flat_map(|f| with_partner(d, mode, f, mirror)).collect();
        let s = &mut self.segments;
        if add && !range {
            let on = !s.selected.contains(field);
            for f in picked {
                if on {
                    s.selected.insert(f);
                } else {
                    s.selected.remove(&f);
                }
            }
        } else {
            s.selected = picked.into_iter().collect();
        }
        if !range {
            s.anchor = Some(field.to_owned());
        }
    }

    /// Picks the segment under `(x, y)` of the box of the ring whose first segment is
    /// `first`, as [`Self::click_segment`] does. A point off the ring changes nothing.
    pub fn click_ring(&mut self, first: &str, x: f32, y: f32, add: bool, range: bool) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let Some((frame, r, _)) = locate(self.description(), mode, first) else { return };
        let Some(ring) = frame.rings.get(r) else { return };
        let hit = segment_at(ring.fields.len(), f64::from(x), f64::from(y));
        if let Some(field) = hit.and_then(|i| ring.fields.get(i)).cloned() {
            self.click_segment(&field, add, range);
        }
    }

    /// Selects every segment of the frame that holds segment `field`.
    pub fn select_all_segments(&mut self, field: &str) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let Some((frame, ..)) = locate(self.description(), mode, field) else { return };
        self.segments.selected = frame.rings.iter().flat_map(|r| r.fields.clone()).collect();
    }

    /// Turns the mirror on or off. Turning it on adds each selected segment's partner.
    pub fn set_mirror(&mut self, on: bool) {
        self.segments.mirror = on;
        let Some((mode, _)) = self.selected_slot() else { return };
        if on {
            let d = self.description();
            let partners: Vec<String> = self
                .segments
                .selected
                .iter()
                .flat_map(|f| with_partner(d, mode, f, true))
                .collect();
            self.segments.selected.extend(partners);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// The colour of `field` in slot 1 of `XInput` as shown.
    fn shown(s: &AppState, field: &str) -> String {
        let mode = controller_core::devices::pro3::XINPUT;
        let slot = s.slot(mode, 1);
        slot.shown().unwrap().setting(field).and_then(Value::as_str).unwrap().to_owned()
    }

    fn picked(s: &AppState) -> Vec<&str> {
        s.segments.selected.iter().map(String::as_str).collect()
    }

    #[test]
    fn clicks_select_one_add_one_or_a_run() {
        let mut s = crate::state::tests::ultimate_2();
        s.click_segment("/lights/custom/2", false, false);
        s.click_segment("/lights/custom/4", false, true);
        assert_eq!(picked(&s), ["/lights/custom/2", "/lights/custom/3", "/lights/custom/4"]);
        s.click_segment("/lights/custom/3", true, false);
        assert_eq!(picked(&s), ["/lights/custom/2", "/lights/custom/4"]);
        s.click_segment("/lights/custom/14", false, true);
        assert_eq!(picked(&s), ["/lights/custom/14"], "a run stays in one ring");
    }

    #[test]
    fn the_mirror_selects_the_same_segment_on_the_other_ring() {
        let mut s = crate::state::tests::ultimate_2();
        s.click_segment("/lights/custom/1", false, false);
        s.set_mirror(true);
        assert_eq!(picked(&s), ["/lights/custom/1", "/lights/custom/13"]);
        s.click_segment("/lights/custom/12", true, false);
        assert_eq!(picked(&s).len(), 4);
    }

    #[test]
    fn the_editor_sets_every_selected_segment() {
        let mut s = crate::state::tests::ultimate_2();
        s.select_all_segments("/lights/custom/0");
        assert_eq!(s.segments.selected.len(), 24);
        s.click_segment("/lights/custom/5", false, false);
        s.set_selection_colour("00ff80");
        s.click_segment("/lights/custom/6", true, false);
        s.set_selection_channel(0, 255.0);
        assert_eq!(
            [shown(&s, "/lights/custom/5"), shown(&s, "/lights/custom/6")],
            ["#ffff80", "#ffff80"]
        );
        s.set_colour("/lights/fire", "#12AB34");
        s.set_colour("/lights/fire", "nope");
        s.set_colour_channel("/lights/fire", 2, 0.0);
        assert_eq!(shown(&s, "/lights/fire"), "#12ab00");
        s.set_colour("/motion/map_to", "#000000");
        assert_eq!(shown(&s, "/motion/map_to"), "off", "no colour there");
        s.set_choice_at("/motion/map_to", 2);
        assert_eq!(shown(&s, "/motion/map_to"), "right_stick");
    }

    #[test]
    fn a_click_finds_the_segment_it_lands_on() {
        // 12 segments of 30 degrees, clockwise from the top.
        let at = |deg: f64| {
            let (x, y) = point((OUTER + INNER) / 2.0, deg);
            segment_at(12, x, y)
        };
        assert_eq!([at(5.0), at(35.0), at(95.0), at(355.0)], [Some(0), Some(1), Some(3), Some(11)]);
        assert_eq!(segment_at(12, 50.0, 50.0), None, "the hole in the middle");
        assert_eq!(segment_at(12, 0.0, 0.0), None, "a corner, off the ring");
    }

    #[test]
    fn a_segment_outline_starts_at_the_top_and_turns_clockwise() {
        let path = segment_path(0, 4);
        assert!(path.starts_with("M 51.28 1.02 A 49 49 0 0 1 98.98 48.72"), "{path}");
        assert!(path.ends_with('Z'));
    }

    #[test]
    fn a_channel_changes_one_byte_and_stays_in_range() {
        assert_eq!(with_channel(0x007E_3200, 1, 300.0), 0x007E_FF00);
        assert_eq!(with_channel(0x007E_3200, 0, -4.0), 0x0000_3200);
        assert_eq!(hex(0x007E_3200), "#7E3200");
        assert_eq!(
            [typed("7e3200"), typed(" #7E3200 "), typed("#7E32")],
            [Some(0x007E_3200), Some(0x007E_3200), None]
        );
    }
}
