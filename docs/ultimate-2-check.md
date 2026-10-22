# Checking an Ultimate 2

This run checks that 8B reads, writes and plays an 8BitDo Ultimate 2 Wireless in
XInput and in DInput. Run it once, at the desk, with the pad on its USB cable. Allow
about 20 minutes. Nothing here retries or resets USB: a failed step stops the run.

The pad keeps the same USB id in both modes and serves only the bank of the mode it is
in. So each phase asks you to move the pad's mode switch, then checks that the pad
agrees before it goes on.

The check writes `fixtures/ultimate-2/check/xinput.json` and `dinput.json` into slot 3
of their modes. Both files hold the same profile:

| Button | Sends |
| --- | --- |
| A | B |
| B | A |
| Y | LB |
| View | Menu |
| RB | nothing |
| PL (left back paddle) | A |
| PR (right back paddle) | R3 |
| X | a macro: Y |
| L4 | a macro: A with LT, then B |

The sticks are swapped. With swap sticks on, the pad plays a macro's trigger on the
other side, so the L4 macro's LT comes out as RT. The physical triggers stay as they
are. Button names are the ones printed on the pad.

## Before you start

1. Close the 8B window and any other tool that talks to the pad.
2. Install 8B's udev rule, so the check can open the pad without root. The app offers
   it the first time it cannot open the pad.
3. Run the check from a terminal in your desktop session. It reads the pad's input
   node, which the session lets its user read.
4. Plug the pad in by cable.
5. Slot 3 must be empty in both modes. The backup refuses to run otherwise: a macro's
   steps live outside the bank, so restoring the bank would not bring them back.

## 1. Back up

```sh
just ultimate-2-check backup
```

For each mode it asks you to switch the pad, reads that mode's bank, and saves it to
`~/.local/state/8b/backups/ultimate-2-check-<mode>.bin`. It refuses to overwrite a
backup: a second one could hold the check's profile. Move an old file away to back up
again.

## 2. Check

```sh
just ultimate-2-check check
```

Add `xinput` or `dinput` to check one mode only, as in `just ultimate-2-check check
dinput`. For each mode:

1. It asks you to switch the pad to the mode.
2. It writes the file into slot 3 with its two macros, in one write.
3. It reads slot 3 back and compares every mapping, setting and macro step with the
   file. It prints each difference and stops if there is one.
4. It tries to write the other mode's file. The pad is not in that mode, so 8B must
   refuse before it sends anything. It checks that the bank did not change.
5. It asks you to select profile 3 with the pad's profile button, and reads the bank to
   confirm the pad plays it.
6. It walks you through 11 presses. For each one it reads what the pad sends on its
   input node and compares it with what the profile says. You do not need a tester.
   For the RB press, which must send nothing, it waits for your Enter. A press that
   sends something else is reported, the presses go on, and the mode fails at the end.

## 3. Restore

```sh
just ultimate-2-check restore
```

For each mode it asks you to switch the pad, writes the backup back, and compares the
bank with it. Run it after a failed check too.

## Report

If you check a pad for the project, open an issue with the terminal output of all three
phases, the pad's firmware version (the check prints it), and both backup files.
