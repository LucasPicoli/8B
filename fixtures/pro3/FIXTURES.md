# Pro 3 Fixture Catalog

## 1. Purpose and Provenance

These fixtures are produced by `controller-core`'s Plan-2a encoders
(`compile_profile` for remap profiles, `encode_macro_steps` and
`encode_macro_metadata` for macros). Each `.blob` and `.steps.bin` is
the device-native byte representation; the corresponding `.json` is the
canonical decoded (human-readable) form. Both are byte-exact against the
C++ oracle and are verified by round-trip tests in `fixture_profiles.rs`
and `fixture_macros.rs`.

To regenerate after changing an encoder, run:

```
cargo test -p controller-core --test fixture_profiles regenerate -- --ignored
cargo test -p controller-core --test fixture_macros regenerate -- --ignored
```

Independent validation is done by loading the device-native bytes into a
gamepad configurator (see Section 2).

---

## 2. How to Validate on a Configurator

### Remap profiles

1. Flash `remap/<mode>-slot<N>.profile.blob` into profile slot `N`.
   - Use the write path once Plan 2c lands, or your existing flashing tool.
2. Open the configurator and switch to profile slot `N`.
3. Confirm every button shows the mapping listed in the table below.

Control-name to configurator mapping reference:

| Canonical name   | Configurator label (typical)           |
|------------------|----------------------------------------|
| `bottom face`    | A (XInput) / B (Switch)                |
| `right face`     | B (XInput) / A (Switch)                |
| `top face`       | Y (XInput) / X (Switch)                |
| `left face`      | X (XInput) / Y (Switch)                |
| `l1`             | LB / L                                 |
| `r1`             | RB / R                                 |
| `l2`             | LT / ZL (analog trigger)               |
| `r2`             | RT / ZR (analog trigger)               |
| `l3`             | LS / L-stick click                     |
| `r3`             | RS / R-stick click                     |
| `select/back`    | Back / Minus                           |
| `start/menu`     | Start / Plus                           |
| `home/guide`     | Guide / Home                           |
| `turbo`          | Turbo (8BitDo-specific)                |
| `screenshot`     | Capture / Screenshot (Switch only)     |
| `lp`             | Back paddle P1                         |
| `rp`             | Back paddle P2                         |
| `l4`             | Back paddle P3                         |
| `r4`             | Back paddle P4                         |
| `disabled`       | -- / None / Disabled                   |

**Note on Switch face buttons:** Switch mode uses an inverted face-button
layout relative to XInput. The canonical names (`bottom face`, `right face`,
etc.) refer to physical position, not the letter label.

### Macros

1. Open the `.json` file next to the `.steps.bin` (same stem) — this is
   the device-truth decoded form and is what a configurator displays.
2. In the configurator's macro editor, navigate to the profile slot and
   macro slot shown in the catalog below.
3. Confirm the activation trigger, repeat settings, and each step's
   duration and button/axis actions match.

---

## 3. Remap Profile Catalog

### 3.1 `xinput-slot1` — RemapX1

**Files:** `remap/xinput-slot1.profile.json` + `.blob`  
**Mode:** xinput | **Profile slot:** 1 | **ID:** `xinput-slot-1-index-0`

#### Button mappings

| Source       | Target       | Category                              |
|--------------|--------------|---------------------------------------|
| right face   | left face    | [XInput face, variant encoding]       |
| bottom face  | top face     | [XInput face, variant encoding]       |
| top face     | bottom face  | [XInput face, variant encoding]       |
| left face    | right face   | [XInput face, variant encoding]       |
| l1           | l1           | identity                              |
| r1           | r1           | identity                              |
| l2           | l2           | identity                              |
| r2           | r2           | identity                              |
| l3           | l3           | identity                              |
| r3           | r3           | identity                              |
| select/back  | select/back  | identity                              |
| start/menu   | start/menu   | identity                              |
| turbo        | turbo        | identity                              |
| home/guide   | home/guide   | [forced identity]                     |
| d-pad up     | d-pad up     | identity                              |
| d-pad down   | d-pad down   | identity                              |
| d-pad left   | d-pad left   | identity                              |
| d-pad right  | d-pad right  | identity                              |
| rp           | disabled     | [back paddle disabled]                |
| lp           | disabled     | [back paddle disabled]                |
| l4           | disabled     | [back paddle disabled]                |
| r4           | disabled     | [back paddle disabled]                |

#### Sticks

| Setting              | Value |
|----------------------|-------|
| left_min_pct         | 20    |
| left_max_pct         | 80    |
| right_min_pct        | 15    |
| right_max_pct        | 85    |
| invert_left_x        | false |
| invert_left_y        | false |
| invert_right_x       | false |
| invert_right_y       | false |
| swap_sticks          | false |
| swap_dpad_with_left_stick | false |

#### Triggers

| Setting         | Value |
|-----------------|-------|
| left_min_pct    | 10    |
| left_max_pct    | 90    |
| right_min_pct   | 5     |
| right_max_pct   | 95    |
| swap_triggers   | false |

#### Vibration

| Channel      | Level |
|--------------|-------|
| left_level   | 4     |
| right_level  | 2     |

---

### 3.2 `xinput-slot2` — RemapX2

**Files:** `remap/xinput-slot2.profile.json` + `.blob`  
**Mode:** xinput | **Profile slot:** 2 | **ID:** `xinput-slot-2-index-1`

#### Button mappings

| Source       | Target       | Category                              |
|--------------|--------------|---------------------------------------|
| right face   | right face   | identity                              |
| bottom face  | bottom face  | identity                              |
| top face     | top face     | identity                              |
| left face    | left face    | identity                              |
| l1           | l1           | identity                              |
| r1           | r1           | identity                              |
| l2           | l2           | identity                              |
| r2           | r2           | identity                              |
| l3           | l3           | identity                              |
| r3           | r3           | identity                              |
| select/back  | disabled     | [select disabled]                     |
| start/menu   | disabled     | [start disabled]                      |
| turbo        | turbo        | identity                              |
| home/guide   | home/guide   | [forced identity]                     |
| d-pad up     | d-pad up     | identity                              |
| d-pad down   | d-pad down   | identity                              |
| d-pad left   | d-pad left   | identity                              |
| d-pad right  | d-pad right  | identity                              |
| rp           | top face     | [back paddle → face button]           |
| lp           | disabled     | [back paddle disabled]                |
| l4           | r1           | [back paddle → shoulder]              |
| r4           | disabled     | [back paddle disabled]                |

#### Sticks

| Setting              | Value |
|----------------------|-------|
| left_min_pct         | 0     |
| left_max_pct         | 100   |
| right_min_pct        | 0     |
| right_max_pct        | 100   |
| invert_left_x        | true  |
| invert_left_y        | false |
| invert_right_x       | false |
| invert_right_y       | true  |
| swap_sticks          | false |
| swap_dpad_with_left_stick | false |

#### Triggers

| Setting         | Value |
|-----------------|-------|
| left_min_pct    | 0     |
| left_max_pct    | 100   |
| right_min_pct   | 0     |
| right_max_pct   | 100   |
| swap_triggers   | false |

#### Vibration

| Channel      | Level |
|--------------|-------|
| left_level   | 5     |
| right_level  | 5     |

---

### 3.3 `xinput-slot3` — RemapX3

**Files:** `remap/xinput-slot3.profile.json` + `.blob`  
**Mode:** xinput | **Profile slot:** 3 | **ID:** `xinput-slot-3-index-2`

#### Button mappings

| Source       | Target       | Category                              |
|--------------|--------------|---------------------------------------|
| right face   | right face   | identity                              |
| bottom face  | bottom face  | identity                              |
| top face     | top face     | identity                              |
| left face    | left face    | identity                              |
| l1           | r1           | [shoulder swap]                       |
| r1           | l1           | [shoulder swap]                       |
| l2           | r2           | [trigger swap]                        |
| r2           | l2           | [trigger swap]                        |
| l3           | r3           | [stick click swap]                    |
| r3           | l3           | [stick click swap]                    |
| select/back  | select/back  | identity                              |
| start/menu   | start/menu   | identity                              |
| turbo        | turbo        | identity                              |
| home/guide   | home/guide   | [forced identity]                     |
| d-pad up     | d-pad down   | [d-pad inversion]                     |
| d-pad down   | d-pad up     | [d-pad inversion]                     |
| d-pad left   | d-pad right  | [d-pad inversion]                     |
| d-pad right  | d-pad left   | [d-pad inversion]                     |
| rp           | disabled     | [back paddle disabled]                |
| lp           | disabled     | [back paddle disabled]                |
| l4           | disabled     | [back paddle disabled]                |
| r4           | disabled     | [back paddle disabled]                |

#### Sticks

| Setting              | Value |
|----------------------|-------|
| left_min_pct         | 5     |
| left_max_pct         | 95    |
| right_min_pct        | 10    |
| right_max_pct        | 90    |
| invert_left_x        | false |
| invert_left_y        | false |
| invert_right_x       | false |
| invert_right_y       | false |
| swap_sticks          | true  |
| swap_dpad_with_left_stick | false |

#### Triggers

| Setting         | Value |
|-----------------|-------|
| left_min_pct    | 20    |
| left_max_pct    | 80    |
| right_min_pct   | 30    |
| right_max_pct   | 70    |
| swap_triggers   | true  |

#### Vibration

| Channel      | Level |
|--------------|-------|
| left_level   | 1     |
| right_level  | 3     |

---

### 3.4 `switch-slot1` — RemapSW1

**Files:** `remap/switch-slot1.profile.json` + `.blob`  
**Mode:** switch | **Profile slot:** 1 | **ID:** `switch-slot-1-index-0`

#### Button mappings

| Source       | Target       | Category                              |
|--------------|--------------|---------------------------------------|
| right face   | bottom face  | [Switch face, variant encoding]       |
| bottom face  | bottom face  | identity                              |
| top face     | top face     | identity                              |
| left face    | left face    | identity                              |
| l1           | l1           | identity                              |
| r1           | r1           | identity                              |
| l2           | l2           | identity                              |
| r2           | r2           | identity                              |
| l3           | l3           | identity                              |
| r3           | r3           | identity                              |
| select/back  | screenshot   | [capture button, Switch default]      |
| start/menu   | start/menu   | identity                              |
| turbo        | screenshot   | [turbo → screenshot, Switch default]  |
| home/guide   | home/guide   | [forced identity]                     |
| d-pad up     | d-pad up     | identity                              |
| d-pad down   | d-pad down   | identity                              |
| d-pad left   | d-pad left   | identity                              |
| d-pad right  | d-pad right  | identity                              |
| rp           | disabled     | [back paddle disabled]                |
| lp           | disabled     | [back paddle disabled]                |
| l4           | disabled     | [back paddle disabled]                |
| r4           | disabled     | [back paddle disabled]                |

#### Sticks

| Setting              | Value |
|----------------------|-------|
| left_min_pct         | 0     |
| left_max_pct         | 100   |
| right_min_pct        | 0     |
| right_max_pct        | 100   |
| invert_left_x        | false |
| invert_left_y        | false |
| invert_right_x       | false |
| invert_right_y       | false |
| swap_sticks          | false |
| swap_dpad_with_left_stick | false |

#### Triggers (Switch — threshold only)

| Setting              | Value |
|----------------------|-------|
| left_threshold_pct   | 25    |
| right_threshold_pct  | 40    |
| swap_triggers        | true  |

#### Vibration

| Channel      | Level |
|--------------|-------|
| left_level   | 3     |
| right_level  | 3     |

---

### 3.5 `switch-slot2` — RemapSW2

**Files:** `remap/switch-slot2.profile.json` + `.blob`  
**Mode:** switch | **Profile slot:** 2 | **ID:** `switch-slot-2-index-1`

#### Button mappings

| Source       | Target       | Category                              |
|--------------|--------------|---------------------------------------|
| right face   | right face   | identity                              |
| bottom face  | disabled     | [face button disabled]                |
| top face     | screenshot   | [face button → screenshot]            |
| left face    | left face    | identity                              |
| l1           | l1           | identity                              |
| r1           | r1           | identity                              |
| l2           | l2           | identity                              |
| r2           | r2           | identity                              |
| l3           | l3           | identity                              |
| r3           | r3           | identity                              |
| select/back  | select/back  | identity                              |
| start/menu   | start/menu   | identity                              |
| turbo        | l1           | [turbo → shoulder, explicit override] |
| home/guide   | home/guide   | [forced identity]                     |
| d-pad up     | d-pad up     | identity                              |
| d-pad down   | d-pad down   | identity                              |
| d-pad left   | d-pad left   | identity                              |
| d-pad right  | d-pad right  | identity                              |
| rp           | disabled     | [back paddle disabled]                |
| lp           | l1           | [back paddle → shoulder]              |
| l4           | disabled     | [back paddle disabled]                |
| r4           | disabled     | [back paddle disabled]                |

#### Sticks

| Setting              | Value |
|----------------------|-------|
| left_min_pct         | 0     |
| left_max_pct         | 90    |
| right_min_pct        | 0     |
| right_max_pct        | 90    |
| invert_left_x        | false |
| invert_left_y        | false |
| invert_right_x       | false |
| invert_right_y       | false |
| swap_sticks          | false |
| swap_dpad_with_left_stick | false |

#### Triggers (Switch — threshold only)

| Setting              | Value |
|----------------------|-------|
| left_threshold_pct   | 10    |
| right_threshold_pct  | 90    |
| swap_triggers        | false |

#### Vibration

| Channel      | Level |
|--------------|-------|
| left_level   | 2     |
| right_level  | 4     |

---

### 3.6 `switch-slot3` — RemapSW3

**Files:** `remap/switch-slot3.profile.json` + `.blob`  
**Mode:** switch | **Profile slot:** 3 | **ID:** `switch-slot-3-index-2`

#### Button mappings

| Source       | Target       | Category                              |
|--------------|--------------|---------------------------------------|
| right face   | right face   | identity                              |
| bottom face  | bottom face  | identity                              |
| top face     | top face     | identity                              |
| left face    | left face    | identity                              |
| l1           | l1           | identity                              |
| r1           | r1           | identity                              |
| l2           | l2           | identity                              |
| r2           | r2           | identity                              |
| l3           | l3           | identity                              |
| r3           | r3           | identity                              |
| select/back  | select/back  | identity                              |
| start/menu   | start/menu   | identity                              |
| turbo        | screenshot   | [turbo → screenshot, Switch default]  |
| home/guide   | home/guide   | [forced identity]                     |
| d-pad up     | d-pad up     | identity                              |
| d-pad down   | d-pad down   | identity                              |
| d-pad left   | d-pad left   | identity                              |
| d-pad right  | d-pad right  | identity                              |
| rp           | disabled     | [back paddle disabled]                |
| lp           | disabled     | [back paddle disabled]                |
| l4           | disabled     | [back paddle disabled]                |
| r4           | disabled     | [back paddle disabled]                |

#### Sticks

| Setting              | Value |
|----------------------|-------|
| left_min_pct         | 0     |
| left_max_pct         | 100   |
| right_min_pct        | 0     |
| right_max_pct        | 100   |
| invert_left_x        | false |
| invert_left_y        | false |
| invert_right_x       | false |
| invert_right_y       | false |
| swap_sticks          | false |
| swap_dpad_with_left_stick | true  |

#### Triggers (Switch — threshold only)

| Setting              | Value |
|----------------------|-------|
| left_threshold_pct   | 0     |
| right_threshold_pct  | 0     |
| swap_triggers        | false |

#### Vibration

| Channel      | Level |
|--------------|-------|
| left_level   | 5     |
| right_level  | 0     |

---

## 4. Macro Catalog

Filename stems encode slot metadata: `<x|s>-s<profileslot>-m<macroslot>-<name>`.
`x` = xinput, `s` = switch.

**Note on Switch trigger values (l2/r2):** Switch mode stores trigger state
as a single bit in the keys bitmap, not as a separate analog byte. The
encoder reconstructs this as trigger value 255 (full deflection) in the
canonical JSON. This reconstructed value of 255 is authoritative — it is
not the literal 0–255 analog input. `l2` and `r2` are purely analog on
this device; they are never available as digital step-button actions in
macros (which is why the digital-button macros do not use them).

---

### 4.1 `x-s1-m0-buttons` — Buttons (XInput)

**Files:** `macros/x-s1-m0-buttons.json` + `.steps.bin`  
**Mode:** xinput | **Profile slot:** 1 | **Macro slot:** 0  
**Trigger:** `rp` (back paddle P2)  
**Repeat:** count=1, interval=0 ms

| Step | Duration | Buttons pressed          | Axes |
|------|----------|--------------------------|------|
| 1    | 50 ms    | bottom face              | —    |
| 2    | 100 ms   | top face, r1             | —    |
| 3    | 0 ms     | d-pad up, l1             | —    |

---

### 4.2 `x-s1-m3-repeat` — Repeat255 (XInput)

**Files:** `macros/x-s1-m3-repeat.json` + `.steps.bin`  
**Mode:** xinput | **Profile slot:** 1 | **Macro slot:** 3  
**Trigger:** `turbo`  
**Repeat:** count=255, interval=1000 ms

| Step | Duration | Buttons pressed | Axes |
|------|----------|-----------------|------|
| 1    | 40 ms    | l1              | —    |
| 2    | 40 ms    | r1              | —    |

---

### 4.3 `x-s2-m1-allbuttons` — AllButtons (XInput)

**Files:** `macros/x-s2-m1-allbuttons.json` + `.steps.bin`  
**Mode:** xinput | **Profile slot:** 2 | **Macro slot:** 1  
**Trigger:** `l1`  
**Repeat:** count=2, interval=200 ms

| Step | Duration | Buttons pressed                                                                                  | Axes |
|------|----------|--------------------------------------------------------------------------------------------------|------|
| 1    | 30 ms    | start/menu, l3, r3, select/back, top face, left face, d-pad right, d-pad left, d-pad down, d-pad up, l1, r1, bottom face, right face | — |

---

### 4.4 `x-s3-m2-sticks-triggers` — SticksTrig (XInput)

**Files:** `macros/x-s3-m2-sticks-triggers.json` + `.steps.bin`  
**Mode:** xinput | **Profile slot:** 3 | **Macro slot:** 2  
**Trigger:** `r4` (back paddle P4)  
**Repeat:** count=1, interval=0 ms

| Step | Duration | Buttons pressed | Left stick    | Right stick | Triggers (L/R) |
|------|----------|-----------------|---------------|-------------|-----------------|
| 1    | 100 ms   | —               | x=200, y=30   | —           | 255 / 128       |
| 2    | 0 ms     | —               | —             | x=64, y=192 | —               |

---

### 4.5 `s-s1-m0-switch-routing` — SwitchL2 (Switch)

**Files:** `macros/s-s1-m0-switch-routing.json` + `.steps.bin`  
**Mode:** switch | **Profile slot:** 1 | **Macro slot:** 0  
**Trigger:** `l2`  
**Repeat:** count=1, interval=0 ms

**Note:** Switch stores triggers as a single bit in the keys bitmap; the
reconstructed trigger value 255 in the JSON is canonical, not a literal
analog input. `l2`/`r2` are analog-only and cannot be digital step buttons.

| Step | Duration | Buttons pressed | Triggers (L/R) |
|------|----------|-----------------|-----------------|
| 1    | 60 ms    | —               | 255 / 0         |

---

### 4.6 `s-s2-m1-continuous` — Continuous (Switch)

**Files:** `macros/s-s2-m1-continuous.json` + `.steps.bin`  
**Mode:** switch | **Profile slot:** 2 | **Macro slot:** 1  
**Trigger:** `r2`  
**Repeat:** count=4294967295 (infinite / 0xFFFFFFFF), interval=16 ms

| Step | Duration | Buttons pressed | Axes |
|------|----------|-----------------|------|
| 1    | 30 ms    | right face      | —    |
| 2    | 30 ms    | —               | —    |

---

### 4.7 `s-s2-m3-triggervariety` — TrigVariety (Switch)

**Files:** `macros/s-s2-m3-triggervariety.json` + `.steps.bin`  
**Mode:** switch | **Profile slot:** 2 | **Macro slot:** 3  
**Trigger:** `start/menu`  
**Repeat:** count=3, interval=100 ms

**Note:** Step 2 includes a reconstructed trigger value of 255 because
Switch encodes trigger activation as a bit in the keys bitmap. The JSON
value 255 is canonical. `l2`/`r2` are analog-only and are never digital
step buttons.

| Step | Duration | Buttons pressed | Left stick    | Triggers (L/R) |
|------|----------|-----------------|---------------|-----------------|
| 1    | 25 ms    | d-pad left      | —             | —               |
| 2    | 25 ms    | —               | x=10, y=240   | 255 / 0         |

---

### 4.8 `s-s3-m2-maxsteps` — MaxSteps (Switch)

**Files:** `macros/s-s3-m2-maxsteps.json` + `.steps.bin`  
**Mode:** switch | **Profile slot:** 3 | **Macro slot:** 2  
**Trigger:** `select/back`  
**Repeat:** count=1, interval=0 ms

255 steps, each 10 ms, each pressing `bottom face`. Tests the maximum
step count the firmware supports.

| Steps    | Duration each | Buttons pressed |
|----------|---------------|-----------------|
| 1–255    | 10 ms         | bottom face     |

---

## 5. Coverage Matrix

### 5a. Profile slots and modes

| Fixture           | Mode    | Slot 1 | Slot 2 | Slot 3 |
|-------------------|---------|--------|--------|--------|
| xinput-slot1      | xinput  | yes    |        |        |
| xinput-slot2      | xinput  |        | yes    |        |
| xinput-slot3      | xinput  |        |        | yes    |
| switch-slot1      | switch  | yes    |        |        |
| switch-slot2      | switch  |        | yes    |        |
| switch-slot3      | switch  |        |        | yes    |

### 5b. Macro slots

| Fixture                 | Mode    | Slot | Macro 0 | Macro 1 | Macro 2 | Macro 3 |
|-------------------------|---------|------|---------|---------|---------|---------|
| x-s1-m0-buttons         | xinput  | 1    | yes     |         |         |         |
| x-s1-m3-repeat          | xinput  | 1    |         |         |         | yes     |
| x-s2-m1-allbuttons      | xinput  | 2    |         | yes     |         |         |
| x-s3-m2-sticks-triggers | xinput  | 3    |         |         | yes     |         |
| s-s1-m0-switch-routing  | switch  | 1    | yes     |         |         |         |
| s-s2-m1-continuous      | switch  | 2    |         | yes     |         |         |
| s-s2-m3-triggervariety  | switch  | 2    |         |         |         | yes     |
| s-s3-m2-maxsteps        | switch  | 3    |         |         | yes     |         |

All four macro slots (0–3) are covered in both modes.

### 5c. Remap categories covered

| Category                         | Covered by                              |
|----------------------------------|-----------------------------------------|
| XInput face variant encoding     | xinput-slot1                            |
| Switch face variant encoding     | switch-slot1                            |
| Select/back disabled             | xinput-slot2                            |
| Start/menu disabled              | xinput-slot2                            |
| Face button disabled             | switch-slot2                            |
| Back paddle disabled             | xinput-slot1, xinput-slot3, switch-slot1, switch-slot2, switch-slot3 |
| Back paddle → face button        | xinput-slot2 (rp → top face)           |
| Back paddle → shoulder           | xinput-slot2 (l4 → r1), switch-slot2 (lp → l1) |
| Shoulder swap                    | xinput-slot3                            |
| Trigger swap                     | xinput-slot3                            |
| Stick click swap                 | xinput-slot3                            |
| D-pad inversion                  | xinput-slot3                            |
| Swap sticks                      | xinput-slot3                            |
| Swap d-pad with left stick       | switch-slot3                            |
| Swap triggers                    | switch-slot1, xinput-slot3              |
| Turbo → screenshot (SW default)  | switch-slot1, switch-slot3              |
| Turbo → other (explicit override)| switch-slot2 (turbo → l1)             |
| Select/back → screenshot         | switch-slot1                            |
| Face button → screenshot         | switch-slot2 (top face → screenshot)   |
| Stick axis inversion             | xinput-slot2 (invert_left_x, invert_right_y) |
| Non-default stick deadzone       | xinput-slot1, xinput-slot3              |
| Non-default trigger range        | xinput-slot1, xinput-slot3              |
| Switch trigger threshold         | switch-slot1, switch-slot2, switch-slot3 |
| Forced identity: home/guide      | all 6 profiles                          |

### 5d. Macro features covered

| Feature                               | Covered by                              |
|---------------------------------------|-----------------------------------------|
| Single-fire (count=1)                 | x-s1-m0-buttons, s-s1-m0-switch-routing, x-s3-m2-sticks-triggers, s-s3-m2-maxsteps |
| Finite repeat (count=2–255)           | x-s1-m3-repeat (255), s-s2-m3-triggervariety (3), x-s2-m1-allbuttons (2) |
| Infinite repeat (count=0xFFFFFFFF)    | s-s2-m1-continuous                     |
| Repeat interval > 0                   | x-s1-m3-repeat (1000 ms), x-s2-m1-allbuttons (200 ms), s-s2-m1-continuous (16 ms), s-s2-m3-triggervariety (100 ms) |
| Multi-step sequence                   | x-s1-m0-buttons (3 steps), x-s1-m3-repeat (2 steps), x-s3-m2-sticks-triggers (2 steps), s-s2-m1-continuous (2 steps), s-s2-m3-triggervariety (2 steps) |
| Maximum step count (255 steps)        | s-s3-m2-maxsteps                       |
| Zero-duration step                    | x-s1-m0-buttons (step 3), x-s3-m2-sticks-triggers (step 2) |
| All digital buttons in one step       | x-s2-m1-allbuttons                     |
| Left stick axes                       | x-s3-m2-sticks-triggers, s-s2-m3-triggervariety |
| Right stick axes                      | x-s3-m2-sticks-triggers                |
| Trigger axes (XInput, analog)         | x-s3-m2-sticks-triggers                |
| Trigger axes (Switch, reconstructed)  | s-s1-m0-switch-routing, s-s2-m3-triggervariety |
| Trigger `l2` activation              | s-s1-m0-switch-routing                 |
| Trigger `r2` activation              | s-s2-m1-continuous                     |
| Trigger `turbo` activation            | x-s1-m3-repeat                         |
| Trigger `rp` activation              | x-s1-m0-buttons                        |
| Trigger `r4` activation              | x-s3-m2-sticks-triggers                |
| Trigger `l1` activation              | x-s2-m1-allbuttons                     |
| Trigger `start/menu` activation      | s-s2-m3-triggervariety                 |
| Trigger `select/back` activation     | s-s3-m2-maxsteps                       |
| XInput mode macros                    | x-s1-m0-buttons, x-s1-m3-repeat, x-s2-m1-allbuttons, x-s3-m2-sticks-triggers |
| Switch mode macros                    | s-s1-m0-switch-routing, s-s2-m1-continuous, s-s2-m3-triggervariety, s-s3-m2-maxsteps |
