//! The Buttons tab and the tab strip: the controller views, their button shapes,
//! and the mapping table, all read from the controller description.

use std::rc::Rc;

use controller_core::description::{ControllerDescription, DISABLED_OUTPUT, UNRECOGNISED_OUTPUT};
use controller_core::model::{CanonicalProfile, Mode};
use controller_core::view::{VIEW_FILL_COLOR, VIEW_LINE_COLOR};
use slint::{Color, ComponentHandle as _, Image, Model as _, ModelRc, SharedString, VecModel};

use crate::state::AppState;
use crate::ui::{AppWindow, MapRow, PadView, Section, Spot, Theme};

/// The output `target` of `button` in `profile`. A button with no entry keeps its
/// own press.
fn target<'a>(profile: &'a CanonicalProfile, button: &'a str) -> &'a str {
    profile.button_mappings.iter().find(|m| m.source == button).map_or(button, |m| &m.target)
}

/// The name shown for output `id` in `mode`.
fn output_label(description: &ControllerDescription, mode: Mode, id: &str) -> String {
    if let Some(label) = description.button(id).and_then(|b| b.labels.get(&mode)) {
        return label.clone();
    }
    match id {
        DISABLED_OUTPUT => "Disabled".to_owned(),
        UNRECOGNISED_OUTPUT => "Unknown".to_owned(),
        _ => description
            .mode(mode)
            .and_then(|m| m.extra_outputs.iter().find(|o| o.id == id))
            .map_or_else(|| id.to_owned(), |o| o.label.clone()),
    }
}

/// The outputs the picker offers `button` in `mode`, as (id, label): every button
/// that can be an output, then Disabled, then the mode's extra outputs. The
/// button's default output and its `current` one come first when missing, so
/// Turbo can go back to Turbo and an unrecognised entry shows as Unknown.
#[must_use]
pub fn choices(state: &AppState, mode: Mode, button: &str, current: &str) -> Vec<(String, String)> {
    let description = state.description;
    let extras = description.mode(mode).map(|m| m.extra_outputs.as_slice()).unwrap_or_default();
    let mut list: Vec<String> = description
        .buttons
        .iter()
        .filter(|b| b.can_be_output)
        .map(|b| b.id.clone())
        .chain([DISABLED_OUTPUT.to_owned()])
        .chain(extras.iter().map(|o| o.id.clone()))
        .collect();
    let default = state.defaults.get(&mode).map(|p| target(p, button));
    for id in [default, Some(current)].into_iter().flatten() {
        if !list.iter().any(|o| o == id) {
            list.insert(0, id.to_owned());
        }
    }
    list.into_iter()
        .map(|id| (output_label(description, mode, &id), id))
        .map(|(l, i)| (i, l))
        .collect()
}

/// The button of table row `row` and the output at `choice` in its picker.
#[must_use]
pub fn picked_output(state: &AppState, row: usize, choice: usize) -> Option<(String, String)> {
    let (mode, number) = state.selected_slot()?;
    let button = state.description.buttons.get(row)?;
    let slot = state.slot(mode, number);
    let current = target(slot.shown()?, &button.id);
    let (output, _) = choices(state, mode, &button.id, current).into_iter().nth(choice)?;
    Some((button.id.clone(), output))
}

/// The note under a button that runs macros, such as `Also runs macro 2`.
fn macro_note(profile: &CanonicalProfile, button: &str) -> String {
    let numbers: Vec<String> = (1..)
        .zip(&profile.macro_refs)
        .filter(|(_, m)| m.trigger == button)
        .map(|(n, _): (u32, _)| n.to_string())
        .collect();
    match numbers.as_slice() {
        [] => String::new(),
        [n] => format!("Also runs macro {n}"),
        _ => format!("Also runs macros {}", numbers.join(", ")),
    }
}

/// The tab strip, with a dot on each tab that holds unsaved edits.
#[must_use]
pub fn sections(state: &AppState) -> Vec<Section> {
    let dirty = state.selected_slot().map(|(m, n)| state.slot(m, n).dirty()).unwrap_or_default();
    [
        ("Buttons", dirty.buttons),
        ("Sticks", dirty.sticks),
        ("Triggers", dirty.triggers),
        ("Vibration", dirty.vibration),
    ]
    .into_iter()
    .map(|(name, unsaved)| Section { name: name.into(), unsaved, count: 0 })
    .collect()
}

/// Whether `button`, a remappable button, outputs something other than it does in
/// `mode`'s default profile.
fn remapped(state: &AppState, mode: Mode, button: &str, current: &str) -> bool {
    state.defaults.get(&mode).is_some_and(|d| target(d, button) != current)
}

/// The mapping table of the selected slot: one row per button, in the
/// description's order. Empty for an empty slot.
#[must_use]
pub fn rows(state: &AppState) -> Vec<MapRow> {
    let Some((mode, number)) = state.selected_slot() else { return Vec::new() };
    let slot = state.slot(mode, number);
    let Some(shown) = slot.shown() else { return Vec::new() };
    state
        .description
        .buttons
        .iter()
        .map(|b| {
            let current = target(shown, &b.id);
            let list = choices(state, mode, &b.id, current);
            MapRow {
                label: b.labels.get(&mode).map_or_else(|| b.id.as_str().into(), Into::into),
                note: macro_note(shown, &b.id).into(),
                output: list
                    .iter()
                    .position(|(id, _)| id == current)
                    .map_or(0, |i| i32::try_from(i).unwrap_or(0)),
                outputs: ModelRc::from(Rc::new(
                    list.into_iter().map(|(_, l)| SharedString::from(l)).collect::<VecModel<_>>(),
                )),
                fixed: !b.can_be_remapped,
                remapped: b.can_be_remapped && remapped(state, mode, &b.id, current),
                unknown: current == UNRECOGNISED_OUTPUT,
                changed: slot.pad.as_ref().is_some_and(|p| target(p, &b.id) != current),
            }
        })
        .collect()
}

/// The button shapes of every view, tinted where the output differs from the
/// default profile's.
#[must_use]
pub fn spots(state: &AppState) -> Vec<Spot> {
    let description = state.description;
    let Some((mode, number)) = state.selected_slot() else { return Vec::new() };
    let slot = state.slot(mode, number);
    let Some(shown) = slot.shown() else { return Vec::new() };
    let mut out = Vec::new();
    for (vi, view) in (0_i32..).zip(&description.views) {
        for h in &view.hotspots {
            let Some(row) = description.buttons.iter().position(|b| b.id == h.button) else {
                continue;
            };
            let fixed = description.buttons.get(row).is_some_and(|b| !b.can_be_remapped);
            let current = target(shown, &h.button);
            let mapped = !fixed && remapped(state, mode, &h.button, current);
            let pending = slot.pad.as_ref().is_some_and(|p| target(p, &h.button) != current);
            let output = output_label(description, mode, current).into();
            #[allow(clippy::cast_possible_truncation)]
            let [x0, y0, x1, y1] = h.bounds().map(|v| v as f32);
            out.push(Spot {
                row: i32::try_from(row).unwrap_or(-1),
                view: vi,
                commands: h.path.as_str().into(),
                mapped,
                pending,
                fixed,
                output,
                x0,
                y0,
                x1,
                y1,
            });
        }
    }
    out
}

/// The row of the button under (`x`, `y`) in view `view`, in viewBox units, or -1.
#[must_use]
pub fn hit(description: &ControllerDescription, view: usize, x: f32, y: f32) -> i32 {
    description
        .views
        .get(view)
        .and_then(|v| v.hotspots.iter().find(|h| h.contains(f64::from(x), f64::from(y))))
        .and_then(|h| description.buttons.iter().position(|b| b.id == h.button))
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1)
}

/// `#rrggbb` for `color`.
fn hex(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.red(), color.green(), color.blue())
}

/// Every view of the controller, drawn in the theme's `line` and `fill` colours.
#[must_use]
pub fn views(description: &ControllerDescription, line: Color, fill: Color) -> Vec<PadView> {
    description
        .views
        .iter()
        .map(|v| {
            let svg = v
                .svg_data
                .replace(VIEW_LINE_COLOR, &hex(line))
                .replace(VIEW_FILL_COLOR, &hex(fill));
            let [width, height] = v.view_box;
            PadView {
                image: Image::load_from_svg_data(svg.as_bytes()).unwrap_or_default(),
                width: f32::from(width),
                height: f32::from(height),
            }
        })
        .collect()
}

/// Pushes the views in the window's current theme colours. Call once, and again
/// when the theme changes.
pub fn render_views(description: &ControllerDescription, ui: &AppWindow) {
    let theme = ui.global::<Theme<'_>>();
    let views = views(description, theme.get_art_line().color(), theme.get_art_fill().color());
    ui.set_views(ModelRc::from(Rc::new(VecModel::from(views))));
}

/// Replaces the rows in place when their count holds, so each row's picker keeps
/// its focus.
fn push_rows(ui: &AppWindow, rows: Vec<MapRow>) {
    let current = ui.get_rows();
    if let Some(model) = current.as_any().downcast_ref::<VecModel<MapRow>>() {
        if model.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                model.set_row_data(i, row);
            }
            return;
        }
    }
    ui.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
}

/// Pushes the tab strip, the button shapes and the mapping table.
pub fn render_buttons(state: &AppState, ui: &AppWindow) {
    ui.set_sections(ModelRc::from(Rc::new(VecModel::from(sections(state)))));
    ui.set_spots(ModelRc::from(Rc::new(VecModel::from(spots(state)))));
    push_rows(ui, rows(state));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::state::tests::{connected as read, description};

    fn ids(list: &[(String, String)]) -> Vec<&str> {
        list.iter().map(|(id, _)| id.as_str()).collect()
    }

    #[test]
    fn picker_lists_outputs_then_disabled_then_mode_extras() {
        let s = read(Mode::Switch);
        let list = choices(&s, Mode::Switch, "l1", "l1");
        let names = ids(&list);
        assert_eq!(names.len(), 19, "17 output buttons, Disabled, Screenshot");
        assert_eq!(names.first(), Some(&"right face"));
        assert_eq!(&names[17..], ["disabled", "screenshot"]);
        assert!(!names.contains(&"turbo") && !names.contains(&"rp"));
        assert_eq!(list[0].1, "A", "labels follow the mode");
        assert!(!ids(&choices(&s, Mode::XInput, "l1", "l1")).contains(&"screenshot"));
    }

    #[test]
    fn picker_offers_turbo_its_own_press_and_shows_unknown_first() {
        let s = read(Mode::XInput);
        let turbo = choices(&s, Mode::XInput, "turbo", "disabled");
        assert_eq!(turbo[0], ("turbo".to_owned(), "Turbo".to_owned()));
        let paddle = choices(&s, Mode::XInput, "rp", "disabled");
        assert!(!ids(&paddle).contains(&"rp"), "a paddle's default is Disabled");
        let unknown = choices(&s, Mode::XInput, "l1", UNRECOGNISED_OUTPUT);
        assert_eq!(unknown[0], (UNRECOGNISED_OUTPUT.to_owned(), "Unknown".to_owned()));
        assert_eq!(unknown.len(), 19, "Unknown, 17 output buttons, Disabled");
    }

    #[test]
    fn a_pick_names_the_row_button_and_the_chosen_output() {
        let s = read(Mode::XInput);
        let row = s.description.buttons.iter().position(|b| b.id == "r1").unwrap();
        assert_eq!(picked_output(&s, row, 17), Some(("r1".to_owned(), "disabled".to_owned())));
        assert_eq!(picked_output(&s, row, 99), None);
        assert_eq!(picked_output(&s, 99, 0), None);
    }

    #[test]
    fn rows_mark_fixed_unknown_changed_and_macro_buttons() {
        let mut s = read(Mode::XInput);
        let pad = s.slots.get_mut(&(Mode::XInput, 1)).unwrap().pad.as_mut().unwrap();
        pad.macro_refs.push(controller_core::model::MacroRef {
            trigger: "r1".to_owned(),
            path: "m.json".to_owned(),
        });
        pad.button_mappings[1].target = UNRECOGNISED_OUTPUT.to_owned();
        s.set_output("r1", "disabled");
        let rows = rows(&s);
        let row =
            |id: &str| rows[s.description.buttons.iter().position(|b| b.id == id).unwrap()].clone();
        assert!(row("home/guide").fixed && !row("l1").fixed);
        assert!(row("r1").remapped && !row("l2").remapped && !row("rp").remapped);
        assert!(row("bottom face").unknown);
        assert_eq!(row("bottom face").output, 0);
        assert!(row("r1").changed && !row("l1").changed);
        assert_eq!(row("r1").note, "Also runs macro 1");
        assert_eq!(
            row("r1").outputs.row_data(usize::try_from(row("r1").output).unwrap()).unwrap(),
            "Disabled"
        );
    }

    #[test]
    fn spots_light_outputs_that_differ_from_the_default() {
        let mut s = read(Mode::DInput);
        s.select(2, 2);
        s.start_from_default();
        assert!(spots(&s).iter().all(|p| !p.mapped));
        s.set_output("l4", "bottom face");
        let lit: Vec<_> = spots(&s).into_iter().filter(|p| p.mapped).collect();
        assert_eq!(lit.len(), 2, "L4 is on the front and the back");
        assert!(lit.iter().all(|p| p.output == "A"));
        assert!(lit.iter().all(|p| !p.pending), "nothing is on the controller to differ from");
        let mut s = read(Mode::XInput);
        s.set_output("l2", "disabled");
        let pending: Vec<_> = spots(&s).into_iter().filter(|p| p.pending).collect();
        assert_eq!(pending.len(), 1);
        assert!(pending.iter().all(|p| p.mapped && p.output == "Disabled"));
    }

    #[test]
    fn hit_finds_the_row_of_the_shape_under_the_point() {
        let d = description();
        let row =
            |id: &str| i32::try_from(d.buttons.iter().position(|b| b.id == id).unwrap()).unwrap();
        assert_eq!(hit(d, 0, 195.0, 40.0), row("l4"));
        assert_eq!(hit(d, 1, 365.0, 165.0), row("lp"));
        assert_eq!(hit(d, 0, 1.0, 1.0), -1);
        assert_eq!(hit(d, 9, 195.0, 40.0), -1);
    }

    #[test]
    fn tabs_carry_the_dirty_dots() {
        let mut s = read(Mode::XInput);
        assert!(sections(&s).iter().all(|t| !t.unsaved));
        s.set_output("r1", "disabled");
        let dots: Vec<bool> = sections(&s).iter().map(|t| t.unsaved).collect();
        assert_eq!(dots, [true, false, false, false]);
    }
}
