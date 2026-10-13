//! The Buttons tab and the tab strip: the controller views, their button shapes,
//! and the mapping table, all read from the controller description.

use std::rc::Rc;

use controller_core::description::{ControllerDescription, DISABLED_OUTPUT, UNRECOGNISED_OUTPUT};
use controller_core::model::{CanonicalProfile, MacroDefinition, Mode};
use controller_core::view::{VIEW_FILL_COLOR, VIEW_LINE_COLOR};
use slint::{Color, ComponentHandle as _, Image, Model as _, ModelRc, SharedString, VecModel};

use crate::macros::{label, macro_number, macro_output};
use crate::state::{AppState, SlotState};
use crate::ui::{AppWindow, MapRow, PadView, Section, Spot, Theme};

/// The output `target` of `button` in `profile`. A button with no entry keeps its
/// own press.
pub fn target<'a>(profile: &'a CanonicalProfile, button: &'a str) -> &'a str {
    profile.button_mappings.iter().find(|m| m.source == button).map_or(button, |m| &m.target)
}

/// What `button` sends in `slot` as shown: its macro when it starts one, because the
/// controller then sends the macro only, else its output `target`. `None` for an
/// empty slot.
pub fn current(slot: &SlotState, button: &str) -> Option<String> {
    let profile = slot.shown()?;
    Some(
        slot.macro_started_by(button)
            .map_or_else(|| target(profile, button).to_owned(), macro_output),
    )
}

/// What `button` sends in `slot` on the controller. `None` for an empty slot.
pub fn held_current(slot: &SlotState, button: &str) -> Option<String> {
    let profile = slot.pad.as_ref()?;
    Some(
        slot.held_macro_started_by(button)
            .map_or_else(|| target(profile, button).to_owned(), macro_output),
    )
}

/// The name shown for output `id` in `mode`.
pub fn output_label(description: &ControllerDescription, mode: Mode, id: &str) -> String {
    if let Some(label) = description.button(id).and_then(|b| b.labels.get(&mode)) {
        return label.clone();
    }
    if let Some(n) = macro_number(id) {
        return label(&[], n);
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

/// Like [`output_label`], and a macro output reads as its macro among `macros`, with
/// its name.
pub fn output_text(
    description: &ControllerDescription,
    mode: Mode,
    macros: &[&MacroDefinition],
    id: &str,
) -> String {
    macro_number(id).map_or_else(|| output_label(description, mode, id), |n| label(macros, n))
}

/// The outputs the picker offers `button` in `mode`, as (id, label): each macro of the
/// selected slot when the button can start one, then every button that can be an
/// output, then the mode's extra outputs, then Disabled. The button's default output
/// and its `current` one come first when missing, so Turbo can go back to Turbo and an
/// unrecognised entry shows as Unknown. A macro whose steps are still being read waits
/// to be offered, as picking it would copy them.
#[must_use]
pub fn choices(state: &AppState, mode: Mode, button: &str, current: &str) -> Vec<(String, String)> {
    let description = state.description();
    let extras = description.mode(mode).map(|m| m.extra_outputs.as_slice()).unwrap_or_default();
    let empty = SlotState::default();
    let slot = state.selected_slot().and_then(|k| state.active()?.slots.get(&k)).unwrap_or(&empty);
    let macros = slot.shown_macros();
    let offered =
        description.button(button).is_some_and(|b| b.can_start_macro) && slot.macros_editable();
    let mut slots: Vec<u8> =
        macros.iter().filter_map(|m| m.macro_slot).filter(|_| offered).collect();
    slots.sort_unstable();
    let mut list: Vec<String> = slots
        .into_iter()
        .map(macro_output)
        .chain(description.buttons.iter().filter(|b| b.can_be_output).map(|b| b.id.clone()))
        .chain(extras.iter().map(|o| o.id.clone()))
        .chain([DISABLED_OUTPUT.to_owned()])
        .collect();
    let default = state.defaults().get(&mode).map(|p| target(p, button));
    for id in [default, Some(current)].into_iter().flatten() {
        if !list.iter().any(|o| o == id) {
            list.insert(0, id.to_owned());
        }
    }
    list.into_iter().map(|id| (id.clone(), output_text(description, mode, &macros, &id))).collect()
}

/// The button of table row `row` and the output at `choice` in its picker, or
/// `None` when that output is already in use.
#[must_use]
pub fn picked_output(state: &AppState, row: usize, choice: usize) -> Option<(String, String)> {
    let (mode, number) = state.selected_slot()?;
    let button = state.description().buttons.get(row)?;
    let current = current(&state.slot(mode, number), &button.id)?;
    let (output, _) = choices(state, mode, &button.id, &current).into_iter().nth(choice)?;
    (output != current).then(|| (button.id.clone(), output))
}

/// The tab strip, with a dot on each tab that holds unsaved edits: Buttons, Macros
/// when the controller has macro slots, then the settings tabs.
#[must_use]
pub fn sections(state: &AppState) -> Vec<Section> {
    let description = state.description();
    let selected = state.selected_slot();
    let dirty = selected.map(|(m, n)| state.slot_dirty(m, n)).unwrap_or_default();
    let Some(mode) = selected.map(|(m, _)| m).or_else(|| description.modes.first().map(|m| m.id))
    else {
        return Vec::new();
    };
    let macros = selected.map_or(0, |(m, n)| state.slot(m, n).shown_macros().len());
    let macros = (description.macro_slot_count > 0).then(|| Section {
        name: "Macros".into(),
        unsaved: dirty.macros,
        count: i32::try_from(macros).unwrap_or(i32::MAX),
    });
    let tabs = description.pages(mode).map(|p| Section {
        name: p.label.as_str().into(),
        unsaved: dirty.tab(&p.id),
        count: 0,
    });
    std::iter::once(Section { name: "Buttons".into(), unsaved: dirty.buttons, count: 0 })
        .chain(macros)
        .chain(tabs)
        .collect()
}

/// Whether `button`, a remappable button, outputs something other than it does in
/// `mode`'s default profile.
fn remapped(state: &AppState, mode: Mode, button: &str, current: &str) -> bool {
    state.defaults().get(&mode).is_some_and(|d| target(d, button) != current)
}

/// The mapping table of the selected slot: one row per button, in the
/// description's order. Empty for an empty slot.
#[must_use]
pub fn rows(state: &AppState) -> Vec<MapRow> {
    let Some((mode, number)) = state.selected_slot() else { return Vec::new() };
    let slot = state.slot(mode, number);
    if slot.shown().is_none() {
        return Vec::new();
    }
    state
        .description()
        .buttons
        .iter()
        .map(|b| {
            let current = current(&slot, &b.id).unwrap_or_default();
            let list = choices(state, mode, &b.id, &current);
            MapRow {
                label: b.labels.get(&mode).map_or_else(|| b.id.as_str().into(), Into::into),
                output: list
                    .iter()
                    .position(|(id, _)| *id == current)
                    .map_or(0, |i| i32::try_from(i).unwrap_or(0)),
                outputs: ModelRc::from(Rc::new(
                    list.into_iter().map(|(_, l)| SharedString::from(l)).collect::<VecModel<_>>(),
                )),
                fixed: !b.can_be_remapped,
                remapped: b.can_be_remapped && remapped(state, mode, &b.id, &current),
                unknown: current == UNRECOGNISED_OUTPUT,
                changed: held_current(&slot, &b.id).is_some_and(|held| held != current),
            }
        })
        .collect()
}

/// The button shapes of every view, tinted where the output differs from the
/// default profile's.
#[must_use]
pub fn spots(state: &AppState) -> Vec<Spot> {
    let description = state.description();
    let Some((mode, number)) = state.selected_slot() else { return Vec::new() };
    let slot = state.slot(mode, number);
    if slot.shown().is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (vi, view) in (0_i32..).zip(&description.views) {
        for h in &view.hotspots {
            let Some(row) = description.buttons.iter().position(|b| b.id == h.button) else {
                continue;
            };
            let fixed = description.buttons.get(row).is_some_and(|b| !b.can_be_remapped);
            let current = current(&slot, &h.button).unwrap_or_default();
            let mapped = !fixed && remapped(state, mode, &h.button, &current);
            let pending = held_current(&slot, &h.button).is_some_and(|held| held != current);
            let output = output_text(description, mode, &slot.shown_macros(), &current).into();
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
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::state::tests::{connected as read, description, hold_macros, macro_def};

    use crate::writes::WriteOp;
    use controller_core::devices::pro3::{DINPUT, SWITCH, XINPUT};

    fn ids(list: &[(String, String)]) -> Vec<&str> {
        list.iter().map(|(id, _)| id.as_str()).collect()
    }

    #[test]
    fn picker_lists_outputs_then_mode_extras_then_disabled() {
        let s = read(SWITCH);
        let list = choices(&s, SWITCH, "l1", "l1");
        let names = ids(&list);
        assert_eq!(names.len(), 19, "17 output buttons, Screenshot, Disabled");
        assert_eq!(names.first(), Some(&"right face"));
        assert_eq!(&names[17..], ["screenshot", "disabled"]);
        assert!(!names.contains(&"turbo") && !names.contains(&"rp"));
        assert_eq!(list[0].1, "A", "labels follow the mode");
        assert!(!ids(&choices(&s, XINPUT, "l1", "l1")).contains(&"screenshot"));
    }

    #[test]
    fn picker_offers_the_paddle_outputs_in_dinput_only() {
        let s = read(DINPUT);
        let list = choices(&s, DINPUT, "l1", "l1");
        assert_eq!(
            &ids(&list)[17..],
            ["rp output", "lp output", "l4 output", "r4 output", "disabled"]
        );
        assert_eq!(list[17].1, "PR");
        for mode in [XINPUT, SWITCH] {
            assert!(!ids(&choices(&s, mode, "l1", "l1")).contains(&"rp output"), "{mode}");
        }
    }

    #[test]
    fn picker_offers_turbo_its_own_press_and_shows_unknown_first() {
        let s = read(XINPUT);
        let turbo = choices(&s, XINPUT, "turbo", "disabled");
        assert_eq!(turbo[0], ("turbo".to_owned(), "Turbo".to_owned()));
        let paddle = choices(&s, XINPUT, "rp", "disabled");
        assert!(!ids(&paddle).contains(&"rp"), "a paddle's default is Disabled");
        let unknown = choices(&s, XINPUT, "l1", UNRECOGNISED_OUTPUT);
        assert_eq!(unknown[0], (UNRECOGNISED_OUTPUT.to_owned(), "Unknown".to_owned()));
        assert_eq!(unknown.len(), 19, "Unknown, 17 output buttons, Disabled");
    }

    #[test]
    fn a_pick_names_the_row_button_and_the_chosen_output() {
        let s = read(XINPUT);
        let row = s.description().buttons.iter().position(|b| b.id == "r1").unwrap();
        assert_eq!(picked_output(&s, row, 17), Some(("r1".to_owned(), "disabled".to_owned())));
        assert_eq!(picked_output(&s, row, 99), None);
        assert_eq!(picked_output(&s, 99, 0), None);
    }

    #[test]
    fn rows_mark_fixed_unknown_and_changed_buttons() {
        let mut s = read(XINPUT);
        let pad =
            s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad.as_mut().unwrap();
        pad.button_mappings[1].target = UNRECOGNISED_OUTPUT.to_owned();
        s.set_output("r1", "disabled");
        let rows = rows(&s);
        let row = |id: &str| {
            rows[s.description().buttons.iter().position(|b| b.id == id).unwrap()].clone()
        };
        assert!(row("home/guide").fixed && !row("l1").fixed);
        assert!(row("r1").remapped && !row("l2").remapped && !row("rp").remapped);
        assert!(row("bottom face").unknown);
        assert_eq!(row("bottom face").output, 0);
        assert!(row("r1").changed && !row("l1").changed);
        assert_eq!(
            row("r1").outputs.row_data(usize::try_from(row("r1").output).unwrap()).unwrap(),
            "Disabled"
        );
    }

    /// `XInput` slot 1 with macro slot 0, `Buttons`, on `rp`, as on the test pad.
    fn with_macro() -> AppState {
        let mut s = read(XINPUT);
        let pad =
            s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad.as_mut().unwrap();
        pad.macro_refs.push(controller_core::model::MacroRef {
            trigger_buttons: vec!["rp".to_owned()],
            path: "xinput-slot1-macro0-Buttons.json".to_owned(),
        });
        hold_macros(&mut s, (XINPUT, 1), &[macro_def(0, "Buttons", &["rp"], 1)]);
        s
    }

    /// The row of button `id`.
    fn row_of(s: &AppState, id: &str) -> usize {
        s.description().buttons.iter().position(|b| b.id == id).unwrap()
    }

    /// The label of the output the picker shows for `id` now.
    fn shown(s: &AppState, id: &str) -> String {
        let row = &rows(s)[row_of(s, id)];
        row.outputs.row_data(usize::try_from(row.output).unwrap()).unwrap().to_string()
    }

    #[test]
    fn a_button_with_a_macro_shows_the_macro_as_its_output() {
        let s = with_macro();
        let at = row_of(&s, "rp");
        let row = &rows(&s)[at];
        assert_eq!(row.output, 0, "the macro comes first");
        assert_eq!(row.outputs.row_data(0).unwrap(), "Buttons (Macro 1)");
        assert!(row.remapped && !row.changed);
        let spot = spots(&s).into_iter().find(|p| p.row == i32::try_from(at).unwrap()).unwrap();
        assert!(spot.mapped && !spot.pending);
        assert_eq!(spot.output, "Buttons (Macro 1)");
        assert_eq!(picked_output(&s, at, 0), None, "the macro is already in use");
    }

    #[test]
    fn a_macro_without_a_name_is_just_its_number() {
        let mut s = read(XINPUT);
        hold_macros(&mut s, (XINPUT, 1), &[macro_def(2, "", &[], 1)]);
        let list = choices(&s, XINPUT, "r1", "r1");
        assert_eq!(list[0], ("macro:2".to_owned(), "Macro 3".to_owned()));
        let d = description();
        assert_eq!(output_label(d, XINPUT, "macro:2"), "Macro 3");
    }

    #[test]
    fn the_picker_lists_each_macro_slot_that_holds_one_first_and_only_for_buttons_that_start_one() {
        let mut s = read(XINPUT);
        hold_macros(
            &mut s,
            (XINPUT, 1),
            &[macro_def(0, "Jump", &["rp"], 2), macro_def(2, "Dash", &[], 1)],
        );
        let list = choices(&s, XINPUT, "r1", "r1");
        assert_eq!(
            &list[..2],
            [
                ("macro:0".to_owned(), "Jump (Macro 1)".to_owned()),
                ("macro:2".to_owned(), "Dash (Macro 3)".to_owned())
            ],
            "macro slot 1 is empty and not listed"
        );
        assert_eq!(list.len(), 20, "2 macros, 17 output buttons, Disabled");
        assert!(!ids(&choices(&s, XINPUT, "home/guide", "home/guide"))
            .iter()
            .any(|i| i.starts_with("macro:")));
        // Staged macros count too.
        s.edit_macros(|m| m.push(macro_def(1, "New", &[], 1)));
        let ids: Vec<_> = choices(&s, XINPUT, "r1", "r1").into_iter().map(|c| c.0).collect();
        assert_eq!(&ids[..3], ["macro:0", "macro:1", "macro:2"]);
    }

    #[test]
    fn a_macro_whose_steps_are_still_reading_is_not_offered() {
        let mut s = with_macro();
        s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad_macros[0].loaded = false;
        assert!(!ids(&choices(&s, XINPUT, "r1", "r1")).iter().any(|i| i.starts_with("macro:")));
        assert_eq!(shown(&s, "rp"), "Buttons (Macro 1)", "its own button still shows it");
        s.set_output("r1", "macro:0");
        assert!(!s.slot(XINPUT, 1).unsaved(), "a pick before the steps arrive does nothing");
    }

    #[test]
    fn a_pick_moves_the_button_into_the_macro_and_out_of_the_one_it_started() {
        let mut s = read(XINPUT);
        let pad =
            s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad.as_mut().unwrap();
        pad.macro_refs.push(controller_core::model::MacroRef {
            trigger_buttons: vec!["rp".to_owned(), "lp".to_owned()],
            path: "xinput-slot1-macro0-Jump.json".to_owned(),
        });
        hold_macros(
            &mut s,
            (XINPUT, 1),
            &[macro_def(0, "Jump", &["rp", "lp"], 2), macro_def(2, "Dash", &["l4"], 1)],
        );
        let at = row_of(&s, "rp");
        let (button, output) = picked_output(&s, at, 1).unwrap();
        assert_eq!((button.as_str(), output.as_str()), ("rp", "macro:2"));
        s.set_output(&button, &output);
        assert_eq!(shown(&s, "rp"), "Dash (Macro 3)");
        let slot = s.slot(XINPUT, 1);
        let edited = slot.edited_macros.clone().unwrap();
        assert_eq!(edited[0].trigger_buttons, ["lp"]);
        let mut joined = edited[1].trigger_buttons.clone();
        joined.sort_by_key(|b| row_of(&s, b));
        assert_eq!(edited[1].trigger_buttons, joined, "in the description's order");
        assert_eq!(joined.len(), 2);
        let target = |id: &str| {
            slot.shown()
                .unwrap()
                .button_mappings
                .iter()
                .find(|m| m.source == id)
                .map(|m| m.target.clone())
        };
        assert_eq!(target("rp").as_deref(), Some("disabled"), "a paddle's own output");
        let dirty = s.slot_dirty(XINPUT, 1);
        assert!(dirty.buttons && dirty.macros);
        assert!(rows(&s)[at].changed);
        let job = s.upload_job((XINPUT, 1)).unwrap();
        let WriteOp::Upload { drop_macros, macro_edits, .. } = job.op else { panic!("an upload") };
        assert_eq!((drop_macros.len(), macro_edits.len()), (0, 2), "both macros are written");
        s.discard();
        assert_eq!(shown(&s, "rp"), "Jump (Macro 1)");
    }

    #[test]
    fn another_output_takes_a_picked_button_off_its_new_macro_again() {
        let mut s = with_macro();
        hold_macros(
            &mut s,
            (XINPUT, 1),
            &[macro_def(0, "Buttons", &["rp"], 1), macro_def(1, "Dash", &[], 1)],
        );
        s.set_output("r1", "macro:1");
        s.set_output("r1", "disabled");
        let edited = s.slot(XINPUT, 1).edited_macros.unwrap();
        assert!(edited.iter().all(|m| !m.trigger_buttons.contains(&"r1".to_owned())));
        assert_eq!(shown(&s, "r1"), "Disabled");
        let job = s.upload_job((XINPUT, 1)).unwrap();
        let WriteOp::Upload { macro_edits, .. } = job.op else { panic!("an upload") };
        assert!(macro_edits.is_empty(), "the macros are as the controller holds them");
    }

    #[test]
    fn a_pick_takes_the_button_off_its_macro_and_discard_brings_it_back() {
        let mut s = with_macro();
        let at = row_of(&s, "rp");
        let (button, output) = picked_output(&s, at, 1).unwrap();
        assert_eq!((button.as_str(), output.as_str()), ("rp", "right face"));
        s.set_output(&button, &output);
        assert_eq!(shown(&s, "rp"), "B");
        assert!(rows(&s)[at].changed);
        let refs = s.slot(XINPUT, 1).edited.unwrap().macro_refs;
        assert_eq!(refs.len(), 1, "the macro stays");
        assert!(refs[0].trigger_buttons.is_empty(), "with no button");
        assert!(s.slot_dirty(XINPUT, 1).buttons);
        s.discard();
        assert_eq!(rows(&s)[at].outputs.row_data(0).unwrap(), "Buttons (Macro 1)");
    }

    #[test]
    fn spots_light_outputs_that_differ_from_the_default() {
        let mut s = read(DINPUT);
        s.select(2, 2);
        s.start_from_default();
        assert!(spots(&s).iter().all(|p| !p.mapped));
        s.set_output("l4", "bottom face");
        let lit: Vec<_> = spots(&s).into_iter().filter(|p| p.mapped).collect();
        assert_eq!(lit.len(), 2, "L4 is on the front and the back");
        assert!(lit.iter().all(|p| p.output == "A"));
        assert!(lit.iter().all(|p| !p.pending), "nothing is on the controller to differ from");
        let mut s = read(XINPUT);
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
        let mut s = read(XINPUT);
        assert!(sections(&s).iter().all(|t| !t.unsaved));
        s.set_output("r1", "disabled");
        let dots: Vec<bool> = sections(&s).iter().map(|t| t.unsaved).collect();
        assert_eq!(dots, [true, false, false, false, false]);
        let names: Vec<String> = sections(&s).iter().map(|t| t.name.to_string()).collect();
        assert_eq!(&names[..2], ["Buttons", "Macros"]);
    }
}
