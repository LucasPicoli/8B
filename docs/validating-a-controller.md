# Validating a controller

Run the six checks below on your own pad, then open an issue with the result. This
guide is for the owner of an Ultimate 2.4G or an Ultimate Wired. The app refuses both
until someone has checked them on a real pad.

The risk: a wrong write can leave a profile slot unreadable on the pad. The app saves
the old slot before every write. Do not unplug the pad while a write runs.

## Set up

1. Close any other tool that talks to the pad.
2. Start the app with the override set. From a source checkout:

   ```sh
   EIGHTB_UNVALIDATED=1 RUST_LOG=info cargo run -p gui
   ```

   With the AppImage, put the same two variables in front of its path. Keep the
   terminal output: the report needs it.
3. Plug the pad in by cable. 8B does not use the 2.4G receiver or Bluetooth.

With `EIGHTB_UNVALIDATED=1`, the app treats an unchecked model as supported and saves
the old slot to `backup-<mode>-slot-<N>-before-write-<YYYYMMDD-HHMMSS>.bin` before
every write. The app puts it in `$XDG_STATE_HOME/8b/backups`, which is
`~/.local/state/8b/backups` when the variable is unset. These files are the "before"
images of your report.

## Expected values

Every value in this table is expected, not yet seen on a pad. A pad that disagrees is
a result. Report it.

| | Ultimate Bluetooth (checked) | Ultimate 2.4G | Ultimate Wired |
| --- | --- | --- | --- |
| Locked USB id | `2dc8:3106` | `2dc8:3106` | `2dc8:3106` |
| Locked string | 8BitDo Ultimate Controller | 8BitDo Ultimate wireless Controller for PC | 8BitDo Ultimate Wired Controller |
| Unlocked USB id | `2dc8:6007` | `2dc8:3012` | `2dc8:3011` |
| Face slots 0 to 3 | right, bottom, top, left | bottom, right, left, top | right, bottom, top, left |
| Face prints | Nintendo | Xbox | Xbox |
| Modes (`gamepad_mode`) | XInput 3, Switch 0 | XInput 3, DInput 1 | XInput 3, DInput 1 |
| Four-way D-pad | always | always | firmware 1.02 and up |

Face buttons, by slot. The map code is the default the slot holds, the same in XInput
and DInput:

| Slot | 2.4G button | 2.4G map code | 2.4G macro code | Wired button | Wired map code | Wired macro code |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | bottom, A | `0x2000` | `0x2000` | right, B | `0x1000` | `0x1000` |
| 1 | right, B | `0x1000` | `0x1000` | bottom, A | `0x2000` | `0x2000` |
| 2 | left, X | `0x10` | `0x10` | top, Y | `0x20` | `0x20` |
| 3 | top, Y | `0x20` | `0x20` | left, X | `0x10` | `0x10` |

The macro codes follow the printed letter (A `0x2000`, B `0x1000`, X `0x10`, Y `0x20`),
not the slot, so on the Wired pad a macro "A" step stores `0x2000`.

Defaults in XInput and in DInput, both pads:

1. Face slots: the map codes above.
2. L1, R1, L2, R2, L3, R3, Select, Start: the same button.
3. Star: Turbo `0x10000`. Home, and the four directions: the same button.
4. P1 and P2: none, except on the Wired pad in DInput, where P1 is `0x2000000` and
   P2 is `0x4000000` (slots 18 and 19).

Also expected:

1. Both pads offer Turbo, Auto turbo and Swap on Star. Neither offers Screenshot.
2. Vibration settings exist in XInput only.
3. The Wired pad offers P1 and P2 as outputs in DInput only.
4. Mode 0 is not a mode 8B supports. A read that finds it asks you to switch the pad
   to XInput or DInput.
5. A pad that is already unlocked shows as `2dc8:3012` or `2dc8:3011` on the cable.
   The app finds it unlocked, sends no unlock and offers no relock.

## The six checks

Do them in order. After each one, write down the result for your report.

### 1. Locked id and string

1. Plug in the pad and leave it locked.
2. Run `lsusb -d 2dc8:`.
3. Look for `2dc8:3106` and its product string.
4. Report both, exactly as printed.

Expected: the id and string in the table above. The app, started with the override,
lists the pad as "8BitDo Ultimate · Locked". Without the override it shows "This
8BitDo Ultimate is not tested yet". If the pad does not appear, run
`lsusb -v -d 2dc8:3106` and attach the output. A pad with the id `3106` and a string
not in the table, such as a Pro 2 Wired or a receiver, is not listed on purpose.

### 2. Unlock and read

1. In the app, started with `EIGHTB_UNVALIDATED=1`, pick the pad and press Unlock.
2. Run `lsusb -d 2dc8:` again and note the new id.
3. Report the new id, the terminal lines `controller opened: ...` and `read ... slots`,
   and whether the app lists the slots of each mode or shows an error.

Expected: `2dc8:3012` (2.4G) or `2dc8:3011` (Wired), and the slots of both modes.
If the new id and the string disagree with the table, the id wins. The app opens
the pad as the model that owns the id. Report the pair.

### 3. Relock

1. Use Relock on the pad, or close the app. Closing the app relocks the pad.
2. Run `lsusb -d 2dc8:`.
3. Report whether the id went back to `2dc8:3106`.

Expected: it does. If it does not, unplug the pad and plug it in again, then say so.

### 4. Face slot order

Start the app with the override again, if you closed it, and press Unlock. Checks 4
to 6 all run in the app.

1. Open a button tester, such as `evtest` or `jstest`, on the pad.
2. In the app, remap face slot 0 to L1 in the XInput mode and write it.
3. Press the four face buttons. Note which one now sends L1.
4. Repeat for slots 1, 2 and 3, one write each.
5. Report the physical position of slots 0 to 3.

Expected: bottom, right, left, top on the 2.4G pad. Right, bottom, top, left on the
Wired pad.

### 5. One write round trip with a macro

1. In the app, remap one button in XInput and write the slot.
2. The app reads the slot back after the write. Check that the remap still shows and
   the write button is greyed out.
3. Click Export and save the slot as `after.json`.
4. Add a macro with one step, "A", on a free trigger. Write it.
5. Press the trigger. Report which physical button fires.
6. Repeat the macro with a "B" step.

Expected: the readback matches what you wrote. The macro
codes are in the tables above. Which physical button a macro step fires is not
known for these pads. Report it.

### 6. One DInput remap

1. Put the pad in DInput with its own mode switch.
2. In the app, remap one button in DInput and write the slot.
3. Read the slot back and check it matches.
4. Press the button in a tester. Report what it sends.

Expected: the readback matches and the button sends the new output. On the Wired
pad, P1 and P2 are offered as outputs here.

## Questions for owners

Answer these in the same report.

1. Wired pad: one report says the pad disconnects after 6 seconds. Does yours? Time
   it, plugged in by cable, with the app closed and then open.
2. Any pad: does the locked string disagree with the unlocked id? Report both.
3. Any pad: does the pad report `gamepad_mode` 0 after you switch it? Report the
   switch position.

## Report

1. Open an issue at <https://github.com/LucasPicoli/8B/issues/new>.
2. Name the model and the firmware version.
3. Paste the result of each check, including every value that disagreed with the
   table.
4. Attach the backup files from `~/.local/state/8b/backups`, your `after.json` and
   the terminal output. The files hold your profile names, so rename any you would
   not want to share.

A model is marked checked in the commit that adds your files as a test fixture.
