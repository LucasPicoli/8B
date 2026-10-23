//! The colour settings of a tab, and its rings of coloured segments, such as the
//! Ultimate 2's light rings. The profile holds each colour as `#rrggbb`.
//!
//! A ring is drawn as `n` arcs in a 100 by 100 box, clockwise from the top. The user
//! selects segments, then one editor sets the colour of every selected one.

use std::collections::BTreeSet;
use std::rc::Rc;

use controller_core::description::{self as described, ControllerDescription};
use controller_core::model::{colour, CanonicalProfile, Mode};
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
    /// The selected slot and its edits when a colour picker opened, which Cancel puts back.
    pub before_pick: Option<((Mode, u8), Option<CanonicalProfile>)>,
    /// The segment a Shift click selects from: the last one clicked.
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

/// The colour code `json` holds at `pointer`.
fn code(json: &Value, pointer: &str) -> Option<u32> {
    json.pointer(pointer).and_then(Value::as_str).and_then(colour::parse)
}

/// The red, green and blue bytes of `code`.
const fn channels(code: u32) -> [u8; 3] {
    let [_, r, g, b] = code.to_be_bytes();
    [r, g, b]
}

/// `colour` as a code, `0x00RRGGBB`.
fn code_of(colour: Color) -> u32 {
    u32::from_be_bytes([0, colour.red(), colour.green(), colour.blue()])
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
        red: r.into(),
        green: g.into(),
        blue: b.into(),
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
/// empty when they hold different colours, and its swatch shows the first of them.
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
    }]
}

impl AppState {
    /// Sets the colour at `pointer` in the selected slot to `text`, `#rrggbb` with or
    /// without its `#`. Text that is no colour, or a pointer that names none, changes
    /// nothing.
    pub fn set_colour(&mut self, pointer: &str, text: &str) {
        if let Some(code) = typed(text) {
            self.set_colours(&[pointer.to_owned()], code);
        }
    }

    /// Sets the colour at `pointer` to the one a picker picked.
    pub fn set_colour_rgb(&mut self, pointer: &str, colour: Color) {
        self.set_colours(&[pointer.to_owned()], code_of(colour));
    }

    /// Sets every selected segment to `text`, as [`Self::set_colour`] does.
    pub fn set_selection_colour(&mut self, text: &str) {
        if let Some(code) = typed(text) {
            let selected = self.selected_segments();
            self.set_colours(&selected, code);
        }
    }

    /// Sets every selected segment to the colour a picker picked.
    pub fn set_selection_rgb(&mut self, colour: Color) {
        let selected = self.selected_segments();
        self.set_colours(&selected, code_of(colour));
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

    /// Sets each colour of `pointers` the slot's tabs show to `code`.
    fn set_colours(&mut self, pointers: &[String], code: u32) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let description = self.description();
        let pointers: Vec<&String> =
            pointers.iter().filter(|p| description.is_colour(mode, p)).collect();
        if pointers.is_empty() {
            return;
        }
        self.edit(|p| {
            for pointer in pointers {
                patch(p, pointer, colour::format(code).into());
            }
        });
    }

    /// Adds segment `field` to the selection, or takes it off when it is selected, as a
    /// click does. With `range`, as a Shift click, the run from the anchor to `field` in
    /// its ring is the selection instead.
    pub fn click_segment(&mut self, field: &str, range: bool) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let d = self.description();
        let Some((frame, r, i)) = locate(d, mode, field) else { return };
        let anchor = self.segments.anchor.as_deref().and_then(|a| locate(d, mode, a));
        let run = match anchor {
            Some((_, ar, ai)) if range && ar == r => {
                let fields =
                    frame.rings.get(r).map(|ring| ring.fields.as_slice()).unwrap_or_default();
                fields.get(ai.min(i)..=ai.max(i)).unwrap_or_default().to_vec()
            }
            _ => vec![field.to_owned()],
        };
        let s = &mut self.segments;
        if range {
            s.selected = run.into_iter().collect();
        } else {
            if !s.selected.remove(field) {
                s.selected.insert(field.to_owned());
            }
            s.anchor = Some(field.to_owned());
        }
    }

    /// Picks the segment under `(x, y)` of the box of the ring whose first segment is
    /// `first`, as [`Self::click_segment`] does. A point off the ring unselects every
    /// segment, as a click anywhere else on the page does.
    pub fn click_ring(&mut self, first: &str, x: f32, y: f32, range: bool) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let Some((frame, r, _)) = locate(self.description(), mode, first) else { return };
        let Some(ring) = frame.rings.get(r) else { return };
        let hit = segment_at(ring.fields.len(), f64::from(x), f64::from(y));
        match hit.and_then(|i| ring.fields.get(i)).cloned() {
            Some(field) => self.click_segment(&field, range),
            None => self.clear_segments(),
        }
    }

    /// Selects every segment of the frame that holds segment `field`.
    pub fn select_all_segments(&mut self, field: &str) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let Some((frame, ..)) = locate(self.description(), mode, field) else { return };
        self.segments.selected = frame.rings.iter().flat_map(|r| r.fields.clone()).collect();
    }

    /// Notes the selected slot's edits as a colour picker opens, for [`Self::cancel_pick`].
    pub fn open_picker(&mut self) {
        let Some((mode, number)) = self.selected_slot() else { return };
        let edited = self.slot(mode, number).edited;
        self.segments.before_pick = Some(((mode, number), edited));
    }

    /// Puts back the selected slot's edits from when the colour picker opened: every
    /// colour it picked goes, and a slot with no edits then has none again.
    pub fn cancel_pick(&mut self) {
        let Some((at, edited)) = self.segments.before_pick.take() else { return };
        if self.selected_slot() == Some(at) {
            if let Some(slot) = self.selected_mut() {
                slot.edited = edited;
            }
        }
    }

    /// Unselects every segment.
    pub fn clear_segments(&mut self) {
        self.segments.selected.clear();
        self.segments.anchor = None;
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
    fn a_click_toggles_a_segment_and_shift_selects_a_run() {
        let mut s = crate::state::tests::ultimate_2();
        s.click_segment("/lights/custom/9", false);
        s.click_segment("/lights/custom/2", false);
        assert_eq!(picked(&s), ["/lights/custom/2", "/lights/custom/9"]);
        s.click_segment("/lights/custom/4", true);
        assert_eq!(picked(&s), ["/lights/custom/2", "/lights/custom/3", "/lights/custom/4"]);
        s.click_segment("/lights/custom/3", false);
        assert_eq!(picked(&s), ["/lights/custom/2", "/lights/custom/4"]);
        s.click_segment("/lights/custom/14", true);
        assert_eq!(picked(&s), ["/lights/custom/14"], "a run stays in one ring");
        s.clear_segments();
        assert_eq!(picked(&s), [] as [&str; 0]);
        s.click_segment("/lights/custom/3", true);
        assert_eq!(picked(&s), ["/lights/custom/3"], "unselecting drops the anchor too");
    }

    #[test]
    fn cancel_puts_back_the_colours_from_when_the_picker_opened() {
        let mut s = crate::state::tests::ultimate_2();
        s.select(0, 0);
        s.set_colour("/lights/custom/0", "#112233");
        s.set_colour("/lights/custom/1", "#445566");
        let before = s.slot(controller_core::devices::pro3::XINPUT, 1).edited;
        s.select_all_segments("/lights/custom/0");
        s.open_picker();
        s.set_selection_rgb(Color::from_rgb_u8(0xff, 0, 0));
        assert_eq!(shown(&s, "/lights/custom/1"), "#ff0000");
        s.cancel_pick();
        assert_eq!(
            [shown(&s, "/lights/custom/0"), shown(&s, "/lights/custom/1")],
            ["#112233", "#445566"],
            "each segment gets its own colour back"
        );
        assert_eq!(s.slot(controller_core::devices::pro3::XINPUT, 1).edited, before);
        s.cancel_pick();
        assert_eq!(shown(&s, "/lights/custom/0"), "#112233", "a second Cancel does nothing");
    }

    #[test]
    fn a_click_off_the_ring_unselects_every_segment() {
        let mut s = crate::state::tests::ultimate_2();
        s.select(0, 0);
        s.click_ring("/lights/custom/0", 50.0, 98.0, false);
        assert_eq!(picked(&s), ["/lights/custom/0"], "6 o'clock on the left ring");
        s.click_ring("/lights/custom/0", 50.0, 50.0, false);
        assert_eq!(picked(&s), [] as [&str; 0], "the hole in the middle");
    }

    #[test]
    fn the_editor_sets_every_selected_segment() {
        let mut s = crate::state::tests::ultimate_2();
        s.select_all_segments("/lights/custom/0");
        assert_eq!(s.segments.selected.len(), 24);
        s.clear_segments();
        s.click_segment("/lights/custom/7", false);
        s.set_selection_colour("00ff80");
        s.click_segment("/lights/custom/8", false);
        s.set_selection_rgb(Color::from_rgb_u8(0xff, 0xff, 0x80));
        assert_eq!(
            [shown(&s, "/lights/custom/7"), shown(&s, "/lights/custom/8")],
            ["#ffff80", "#ffff80"]
        );
        s.set_colour("/lights/fire", "#12AB34");
        s.set_colour("/lights/fire", "nope");
        assert_eq!(shown(&s, "/lights/fire"), "#12ab34");
        s.set_colour_rgb("/lights/fire", Color::from_rgb_u8(0x12, 0xab, 0));
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
    fn a_colour_shows_in_upper_case_and_takes_typed_hex() {
        assert_eq!(hex(0x007E_3200), "#7E3200");
        assert_eq!(
            [typed("7e3200"), typed(" #7E3200 "), typed("#7E32")],
            [Some(0x007E_3200), Some(0x007E_3200), None]
        );
    }
}
