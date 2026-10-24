# Checking the Ultimate 2 in the window

This run checks that the 8B window builds every kind of mapping, macro and setting of an
8BitDo Ultimate 2 Wireless, and that the pad holds what the window wrote. You build
slots 2 and 3 of each mode by hand, and a script reads them back and compares each with
its file in `fixtures/ultimate-2/gui-check/`. Allow about 60 minutes. Slot 1 of each
mode stays as it is: the script checks that too.

The script reads the pad and never writes to it, except `restore`. Close the 8B window
before each script phase: the script and the window cannot both hold the pad.

## Before you start

1. Plug the pad in by cable.
2. Slots 2 and 3 must be empty in both modes. The backup refuses to run otherwise.

## 1. Back up

```sh
just ultimate-2-gui-check backup
```

It asks you to switch the pad to each mode and saves that mode's bank to
`~/.local/state/8b/backups/ultimate-2-gui-check-<mode>.bin`.

## 2. XInput slot 2: mappings and settings

Open 8B with the pad in XInput and select slot 2. Name it `Mapas XI çãé 16!`: 16
characters, the limit.

On the Buttons tab:

| Button | Output |
| --- | --- |
| A, B, X, Y | B, A, Y, X |
| LB, RB, LT, RT | RT, LT, LB, Disabled |
| LS, RS | RS, LS |
| View, Menu | Guide, View |
| ↑, ↓, ← | ↓, ↑, Menu |
| PR, PL, L4 | ←, A, Disabled |

→ and R4 stay as they are. L4 reads back as L4: a paddle has no output of its own, so
the pad stores Disabled as "left as it is".

Then the settings:

1. Sticks: left dead zone 0 to 10 %, left Invert X on. Right dead zone 90 to 100 %,
   right Invert Y on. Swap the left and right sticks on.
2. Triggers: left range 0 to 50 %, right range 50 to 100 %. Swap the left and right
   triggers on.
3. Vibration: left motor 0, right motor 5.
4. Motion: Moves the left stick, turned on by D-pad down, Press to toggle,
   sensitivity 1, dead zone 0 %.
5. Lights: Light-tracing `#112233` on `#445566`. Fire Ring `#778899` on `#AABBCC`,
   speed 1. Custom: Select all, type `#0000FF`, OK. Click the left ring's top segment,
   Shift-click the segment left of it to select the whole ring, type `#FF0000`, OK.
   Then the left ring's 6 o'clock segment and the right ring's 12 o'clock segment, each
   `#FFFFFF`.

Do not write yet.

## 3. XInput slot 3: macros

Select slot 3 and name it `Macros XI`. The settings:

1. Sticks: left dead zone 0 to 100 %. Swap the D-pad and the left stick on.
2. Vibration: left motor 1, right motor 4.
3. Motion: Moves the right stick, turned on by LB, Hold the button, sensitivity 10,
   dead zone 100 %.
4. Lights: Light-tracing `#000000` on `#FFFFFF`. Fire Ring `#FF8000` on `#000000`,
   speed 15. Custom: Select all, type `#00FF00`, OK.

On the Macros tab, a step with nothing pressed shows as Pause. Each step's length is
its Duration.

| Macro | Name | Started by | Plays | Steps |
| --- | --- | --- | --- | --- |
| 1 | `Uma vez` | Y and X | Once | 100 ms A and LT; 100 ms B and RT; 50 ms pause; 100 ms LB, RB and ↑ |
| 2 | `Três vezes` | Turbo (Star) | 3 times, 250 ms pause | 200 ms left stick X 0 Y 127, Glide to step 2 on; 100 ms left stick X 255 Y 127; 100 ms pause |
| 3 | `Até apertar` | PR | Until pressed again | 100 ms View and Menu with the right stick X 127 Y 0; 100 ms pause |
| 4 | `408 passos` | L4 | Once | Import macro… `fixtures/ultimate-2/gui-check/long-macro.json`: 408 steps |

If the imported macro is not started by L4, add L4.

## 4. Write and export XInput

1. Write both slots with Write all 2 slots… in the sidebar. Macro 2 moves the left stick
   while its dead zone starts under 2 %, so the window warns that the pad may drop the
   move. Write anyway.
2. Export slot 2 as `xi2.json` and slot 3 as `xi3.json`, in a folder of your own.
3. Close 8B and run `just ultimate-2-gui-check check xinput`. Fix any slot it names,
   then run it again.

## 5. DInput slots 2 and 3: import

Switch the pad to DInput and open 8B.

1. Slot 2: Import… `xi2.json`. Rename it `Mapas DI`. Set PR to RB.
2. Slot 3: Import… `xi3.json`. Rename it `Macros DI`. Delete macro 2: a firmware bug
   erases that macro slot in this slot of DInput, and the window warns on its row.
   Rename macro 1 `Cinco vezes` and set it to play 5 times.
3. Write both slots, then close 8B.

## 6. Check

```sh
just ultimate-2-gui-check check
```

It asks you to switch the pad to each mode, then compares slots 2 and 3 with their files
and slot 1 with the backup. It names every difference before it fails.

## 7. Clear

Open 8B and use Clear slot… on slots 2 and 3 of each mode. Close 8B, then:

```sh
just ultimate-2-gui-check cleared
```

It checks that the four slots hold no profile, and that slot 1 is as the backup has it.
Clear slot keeps a slot's macros stored, as its question says, so the phase lists them
without failing. The next profile written to the slot removes them.

## Restore

If a phase fails and the pad holds a slot you do not want, put the backups back:

```sh
just ultimate-2-gui-check restore
```
