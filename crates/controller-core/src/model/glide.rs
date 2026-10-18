//! Glides: a step whose sticks and triggers move in a straight line to the next step.
//!
//! The pad holds fixed values per step, so a glide lives only in the model and in macro
//! files. [`expand`] turns each gliding step into short steps before a write, and
//! [`fold`] turns a run of short steps back into one gliding step after a read. The
//! rules are the same for every model; only the short step length differs, and it comes
//! from the controller description (`limits.glide_step_ms`). A controller without one
//! has no glides: [`expand`] only drops glide marks and [`fold`] changes nothing.
//!
//! `expand(fold(x)) == x` for any steps `x`, so the pad bytes round-trip. `fold` is
//! stable after one pass, so a file round-trips once its glides are in canonical form.

use std::collections::BTreeSet;

use super::macros::{GlideControl, MacroDefinition, MacroStep};

/// The fewest short steps that fold into a glide. A shorter run reads as plain steps.
pub const MIN_FOLD_RUN: usize = 3;

/// The values of `control` in `step`: x and y of a stick, or a trigger twice.
#[must_use]
pub const fn axes(step: &MacroStep, control: GlideControl) -> [u8; 2] {
    match control {
        GlideControl::LeftStick => [step.left_stick_x, step.left_stick_y],
        GlideControl::RightStick => [step.right_stick_x, step.right_stick_y],
        GlideControl::LeftTrigger => [step.trigger_left; 2],
        GlideControl::RightTrigger => [step.trigger_right; 2],
    }
}

/// Sets `control` in `step` to `[x, y]`, or `[v, _]` for a trigger.
pub const fn set_axes(step: &mut MacroStep, control: GlideControl, [a, b]: [u8; 2]) {
    match control {
        GlideControl::LeftStick => (step.left_stick_x, step.left_stick_y) = (a, b),
        GlideControl::RightStick => (step.right_stick_x, step.right_stick_y) = (a, b),
        GlideControl::LeftTrigger => step.trigger_left = a,
        GlideControl::RightTrigger => step.trigger_right = a,
    }
}

/// The largest change of a gliding control of `step` to its value in `next`.
fn largest_change(step: &MacroStep, next: &MacroStep) -> u8 {
    step.glide
        .iter()
        .flat_map(|&c| axes(step, c).into_iter().zip(axes(next, c)))
        .map(|(from, to)| from.abs_diff(to))
        .max()
        .unwrap_or(0)
}

/// How many short steps the glide of `step` to `next` becomes: one per `glide_step_ms`
/// of its duration, at most one per value of change, at least one. One without glides.
fn short_steps(step: &MacroStep, next: &MacroStep, glide_step_ms: Option<u16>) -> usize {
    glide_step_ms.map_or(1, |ms| {
        usize::from(step.duration_ms)
            .div_ceil(usize::from(ms.max(1)))
            .min(usize::from(largest_change(step, next)))
            .max(1)
    })
}

/// `a` moved `k` of `n` of the way to `b`, rounded half up.
fn lerp(a: u8, b: u8, k: usize, n: usize) -> u8 {
    let (a, b) = (usize::from(a), usize::from(b));
    let value = (2 * (a * n.saturating_sub(k) + b * k) + n) / (2 * n.max(1));
    u8::try_from(value).unwrap_or(u8::MAX)
}

/// The short steps of `step`, which glides to `next`. A step with no glide is itself.
fn expand_step(step: &MacroStep, next: &MacroStep, glide_step_ms: Option<u16>) -> Vec<MacroStep> {
    let n = short_steps(step, next, glide_step_ms);
    let each = usize::from(step.duration_ms) / n;
    let last = usize::from(step.duration_ms) - each * (n - 1);
    (0..n)
        .map(|k| {
            let mut short = MacroStep {
                duration_ms: u16::try_from(if k + 1 == n { last } else { each })
                    .unwrap_or(u16::MAX),
                glide: BTreeSet::new(),
                ..step.clone()
            };
            for &control in &step.glide {
                let ([fx, fy], [tx, ty]) = (axes(step, control), axes(next, control));
                set_axes(&mut short, control, [lerp(fx, tx, k, n), lerp(fy, ty, k, n)]);
            }
            short
        })
        .collect()
}

/// `def` with every gliding step turned into the short steps the pad plays.
///
/// A step that glides gets short steps of about `glide_step_ms`, at most one per value
/// of change. The last step has no next step to glide to and stays as one step.
#[must_use]
pub fn expand(def: &MacroDefinition, glide_step_ms: Option<u16>) -> MacroDefinition {
    let mut steps = Vec::with_capacity(def.steps.len());
    for (step, next) in def.steps.iter().zip(def.steps.iter().skip(1).map(Some).chain([None])) {
        match next {
            Some(next) if !step.glide.is_empty() => {
                steps.extend(expand_step(step, next, glide_step_ms));
            }
            _ => steps.push(MacroStep { glide: BTreeSet::new(), ..step.clone() }),
        }
    }
    MacroDefinition { steps, ..def.clone() }
}

/// How many steps [`expand`] gives, without building them.
#[must_use]
pub fn expanded_len(steps: &[MacroStep], glide_step_ms: Option<u16>) -> usize {
    steps
        .iter()
        .zip(steps.iter().skip(1).map(Some).chain([None]))
        .map(|(step, next)| match next {
            Some(next) if !step.glide.is_empty() => short_steps(step, next, glide_step_ms),
            _ => 1,
        })
        .sum()
}

/// `def` with each run of short steps that [`expand`] would give folded into one gliding
/// step.
///
/// A run is [`MIN_FOLD_RUN`] or more steps that hold the same buttons, where a stick or
/// trigger moves, and where expanding the folded step gives exactly the same steps. The
/// longest run wins, so two glides at the same rate become one. A step that already
/// glides is kept as it is.
#[must_use]
pub fn fold(def: &MacroDefinition, glide_step_ms: Option<u16>) -> MacroDefinition {
    if glide_step_ms.is_none() {
        return def.clone();
    }
    let steps = &def.steps;
    let mut out = Vec::with_capacity(steps.len());
    let mut i = 0;
    while let Some(first) = steps.get(i) {
        let same = steps
            .iter()
            .skip(i)
            .take_while(|s| s.glide.is_empty() && s.pressed_buttons == first.pressed_buttons)
            .count();
        let longest = same.min(steps.len() - i - 1);
        let folded = (MIN_FOLD_RUN..=longest)
            .rev()
            .find_map(|len| fold_run(steps.get(i..i + len)?, steps.get(i + len)?, glide_step_ms));
        if let Some((step, len)) = folded {
            out.push(step);
            i += len;
        } else {
            out.extend(steps.get(i).cloned());
            i += 1;
        }
    }
    MacroDefinition { steps: out, ..def.clone() }
}

/// The gliding step that expands to `run`, which glides to `next`, and the length of `run`.
fn fold_run(
    run: &[MacroStep],
    next: &MacroStep,
    glide_step_ms: Option<u16>,
) -> Option<(MacroStep, usize)> {
    let first = run.first()?;
    let glide: BTreeSet<GlideControl> = [
        GlideControl::LeftStick,
        GlideControl::RightStick,
        GlideControl::LeftTrigger,
        GlideControl::RightTrigger,
    ]
    .into_iter()
    .filter(|&c| run.iter().any(|s| axes(s, c) != axes(first, c)))
    .collect();
    if glide.is_empty() {
        return None;
    }
    let duration_ms =
        u16::try_from(run.iter().map(|s| u32::from(s.duration_ms)).sum::<u32>()).ok()?;
    let step = MacroStep { duration_ms, glide, ..first.clone() };
    (expand_step(&step, next, glide_step_ms) == run).then_some((step, run.len()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::devices::pro3::XINPUT;

    const MS: Option<u16> = Some(20);

    fn def(steps: Vec<MacroStep>) -> MacroDefinition {
        MacroDefinition {
            name: "Glide".into(),
            mode: XINPUT,
            trigger_buttons: Vec::new(),
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps,
            macro_slot: None,
        }
    }

    fn at(duration_ms: u16, left_stick_x: u8) -> MacroStep {
        MacroStep { duration_ms, left_stick_x, ..MacroStep::default() }
    }

    fn gliding(duration_ms: u16, left_stick_x: u8) -> MacroStep {
        MacroStep { glide: [GlideControl::LeftStick].into(), ..at(duration_ms, left_stick_x) }
    }

    fn xs(def: &MacroDefinition) -> Vec<(u16, u8)> {
        def.steps.iter().map(|s| (s.duration_ms, s.left_stick_x)).collect()
    }

    #[test]
    fn a_controller_without_glides_keeps_every_step() {
        let ramp = expand(&def(vec![gliding(100, 0), at(10, 90)]), MS);
        assert_eq!(fold(&ramp, None), ramp, "no fold");
        let marked = def(vec![gliding(100, 0), at(10, 90)]);
        assert_eq!(xs(&expand(&marked, None)), [(100, 0), (10, 90)]);
        assert!(expand(&marked, None).steps.iter().all(|s| s.glide.is_empty()));
        assert_eq!(expanded_len(&marked.steps, None), 2);
    }

    #[test]
    fn a_glide_becomes_short_steps_with_the_remainder_on_the_last() {
        // 50 ms is 3 short steps of 16, 16 and 18 ms; 0 to 90 moves 0, 30, 60 and the
        // next step holds 90.
        let long = expand(&def(vec![gliding(50, 0), at(10, 90)]), MS);
        assert_eq!(xs(&long), [(16, 0), (16, 30), (18, 60), (10, 90)]);
        assert!(long.steps.iter().all(|s| s.glide.is_empty()));
    }

    #[test]
    fn a_glide_has_at_most_one_short_step_per_value_of_change() {
        // 1 s would be 50 steps, but 0 to 4 changes by 4.
        let long = expand(&def(vec![gliding(1000, 0), at(0, 4)]), MS);
        assert_eq!(xs(&long), [(250, 0), (250, 1), (250, 2), (250, 3), (0, 4)]);
        assert_eq!(expanded_len(&def(vec![gliding(1000, 0), at(0, 4)]).steps, MS), 5);
    }

    #[test]
    fn a_stick_and_a_trigger_glide_in_the_same_step() {
        let steps = vec![
            MacroStep {
                glide: [GlideControl::RightStick, GlideControl::RightTrigger].into(),
                duration_ms: 60,
                right_stick_y: 100,
                ..MacroStep::default()
            },
            MacroStep { right_stick_y: 160, trigger_right: 60, ..MacroStep::default() },
        ];
        let long = expand(&def(steps), MS);
        let got: Vec<_> = long.steps.iter().map(|s| (s.right_stick_y, s.trigger_right)).collect();
        assert_eq!(got, [(100, 0), (120, 20), (140, 40), (160, 60)]);
        assert_eq!(fold(&long, MS).steps[0].glide.len(), 2);
    }

    #[test]
    fn the_last_step_cannot_glide() {
        assert_eq!(xs(&expand(&def(vec![gliding(40, 9)]), MS)), [(40, 9)]);
    }

    #[test]
    fn a_run_of_three_short_steps_folds_and_two_do_not() {
        let three = def(vec![at(20, 0), at(20, 30), at(20, 60), at(0, 90)]);
        let folded = fold(&three, MS);
        assert_eq!(xs(&folded), [(60, 0), (0, 90)]);
        assert_eq!(folded.steps[0].glide, [GlideControl::LeftStick].into());

        let two = def(vec![at(20, 0), at(20, 45), at(0, 90)]);
        assert_eq!(fold(&two, MS), two, "2 steps stay plain steps");
    }

    #[test]
    fn an_even_ramp_of_plain_steps_folds_on_the_first_read() {
        let ramp = def(vec![at(16, 0), at(16, 30), at(18, 60), at(10, 90)]);
        let once = fold(&ramp, MS);
        assert_eq!(xs(&once), [(50, 0), (10, 90)]);
        assert_eq!(fold(&once, MS), once, "stable after one pass");
    }

    #[test]
    fn two_glides_at_the_same_rate_fold_into_one() {
        // 0 to 60 over 3 steps, then 60 to 120 over 3 steps: one glide of 6 steps.
        let two = def(vec![gliding(60, 0), gliding(60, 60), at(0, 120)]);
        let folded = fold(&expand(&two, MS), MS);
        assert_eq!(xs(&folded), [(120, 0), (0, 120)]);
    }

    #[test]
    fn a_run_stops_where_the_buttons_change() {
        let mut steps = vec![at(20, 0), at(20, 30), at(20, 60), at(20, 90), at(0, 120)];
        steps[3].pressed_buttons = vec!["l1".into()];
        let folded = fold(&def(steps), MS);
        assert_eq!(xs(&folded), [(60, 0), (20, 90), (0, 120)]);
    }

    /// Steps from `seed`: values drawn from a few levels so ramps and plain steps both
    /// turn up, with glides only where a value changes.
    fn random_steps(seed: &mut u64) -> Vec<MacroStep> {
        let mut next = |m: usize| {
            *seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from((*seed >> 33) % u64::try_from(m).unwrap()).unwrap()
        };
        let len = 1 + next(7);
        let mut steps: Vec<MacroStep> = (0..len)
            .map(|_| MacroStep {
                duration_ms: [0, 20, 20, 40, 50, 100, 1000][next(7)],
                left_stick_x: [0, 127, 200, 255][next(4)],
                left_stick_y: [127, 127, 0, 90][next(4)],
                trigger_right: [0, 0, 255, 17][next(4)],
                pressed_buttons: if next(5) == 0 { vec!["l1".into()] } else { Vec::new() },
                ..MacroStep::default()
            })
            .collect();
        for i in 0..steps.len().saturating_sub(1) {
            let (a, b) = (steps[i].clone(), steps[i + 1].clone());
            for (control, moves) in [
                (
                    GlideControl::LeftStick,
                    axes(&a, GlideControl::LeftStick) != axes(&b, GlideControl::LeftStick),
                ),
                (GlideControl::RightTrigger, a.trigger_right != b.trigger_right),
            ] {
                if moves && next(2) == 0 {
                    steps[i].glide.insert(control);
                }
            }
        }
        steps
    }

    #[test]
    fn expand_undoes_fold_and_fold_is_stable_after_one_pass() {
        let mut seed = 7;
        for _ in 0..2000 {
            let y = def(random_steps(&mut seed));
            let x = expand(&y, MS);
            assert_eq!(expanded_len(&y.steps, MS), x.steps.len());
            assert_eq!(expand(&fold(&x, MS), MS), x, "the pad steps round-trip: {y:?}");
            let canonical = fold(&x, MS);
            assert_eq!(fold(&expand(&canonical, MS), MS), canonical, "stable: {y:?}");
        }
    }
}
