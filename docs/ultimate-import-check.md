# Checking an imported profile on an Ultimate

This run checks that the Ultimate Bluetooth takes a profile file with macros and plays
it, in XInput and in Switch mode. Run it once, at the desk, with the pad on its USB
cable. Allow about 30 minutes. Nothing here retries or resets USB: a failed step stops
the run.

The files are `fixtures/ultimate/import/xinput.json` and `switch.json`. Each goes into
slot 3 of its mode, so the profile in slot 1 stays alone. Each remaps six buttons and
holds two macros: LS plays buttons, RS plays stick directions.

| Mode | Remaps | LS plays | RS plays |
| --- | --- | --- | --- |
| XInput | X to A, L to RB, − to Menu, P1 to Y, P2 to Turbo, Star to Swap | A, B, Y | left up, left right, right down, right left |
| Switch | Y to A, R to L, + to −, P2 to B, P1 to Turbo, Star to Screenshot | X, Y, L | left down, left left, right up, right right |

Button names are the ones printed on the pad. The tester names the outputs.

## Before you start

1. Close the 8B window. The check opens the pad itself.
2. Open a gamepad tester that names buttons, such as `evtest`, for the presses later.
3. Plug the pad in by cable, locked (it shows as `2dc8:3106`).

## 1. Back up

```
just ultimate-import-check backup
```

It unlocks the pad, reads both banks, saves them to
`~/.local/state/8b/backups/ultimate-import-check.bin`, and relocks. It refuses to
overwrite that file: a second backup would hold the imported profile. Move the old file
away if you need to run it again.

## 2. Import and write, in the 8B app

For each mode:

1. Start 8B and unlock the pad.
2. Select the mode, then slot 3.
3. Import the file of that mode, review the notes, and write.
4. Close 8B, so the pad relocks.

## 3. Check

```
just ultimate-import-check check
```

1. It unlocks, reads both banks, and compares slot 3 with each file. It prints every
   difference (mapping, macro trigger, macro step) and stops if there is one. It
   relocks the pad.
2. For each mode it asks you to select the mode and slot 3 on the pad. The repo does
   not say how the pad picks its mode or slot, so use the pad's own way. The check
   cannot confirm the selection.
3. It then prompts each press: what to press, and what the tester should show. Answer
   with Enter for a pass or `n` for a fail. There is no timeout. The first `n` stops
   the run.

The Turbo and Swap prompts change the pad's state until you undo them. The prompts
include the undo.

## 4. Restore

```
just ultimate-import-check restore
```

It unlocks, writes the saved banks back, reads them again, compares each bank with the
backup, and relocks. It ignores byte 0x12, which the pad fills in itself.

If a phase panics before it relocks, unplug the pad and plug it in again. Do not reset
USB by hand.
