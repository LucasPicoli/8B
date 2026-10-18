# Ultimate fixtures

Every `.bin` here is one 1652-byte profile image of an 8BitDo Ultimate (the variant
with Nintendo labels and a 2.4G/Bluetooth switch), firmware 1.09, over its USB cable.
`crates/controller-core/tests/ultimate/` decodes each one and encodes it back. Every
compare skips the blob CRC at `0x00C`..`0x00F`, which the pad does not check.

## Read from the pad

- `xinput-read.bin`: the first read of the `XInput` image. Slot 1 holds the profile an
  earlier vendor-app save left (Star on Auto turbo); slots 2 and 3 are empty.
- `switch-erased.bin`: the Switch image before any save. Every byte is `ff` except
  the image's mode (`00 00`) and the selection (`06 00`).

- `firmware-reply.txt`: the pad's reply to the firmware request `81 05 00 21 01`, in
  hex, from a USB capture of the vendor app. Bytes 2 and 3, `6d 00`, are the version
  times 100: 1.09.

- `unlock-relock.txt`: the unlock sequence the vendor app sent to the locked pad's
  interrupt OUT endpoint `0x02`, and the relock it sent to the config port, with their
  times, from two USB captures on Windows.

## Saved by the vendor app

Each file is the full image of one save, rebuilt from its 37 write packets in a USB
capture of the vendor app on Windows.

- `saves/01.bin` to `saves/09.bin`: nine saves in a row into slot 2 of the `XInput`
  image, starting from `xinput-read.bin`. Each changes one thing:
  1. The button map (printed A sends Xbox A, the bumpers, triggers, sticks, menu
     buttons and D-pad swapped, P1 on Home, P2 on Turbo).
  2. Stick ranges (left 63..128, right 0..64) and toggles `0x201f`.
  3. Trigger ranges (left 118..255, right 0..119) and the trigger swap.
  4. Vibration: left level 0, right level 2.
  5. A macro on printed A. 6. The same bytes again, after the vendor app reported a
     failure. 7. That macro deleted. 8. A macro on P1. 9. A macro on P2.

  The vendor app also wrote things 8B never writes: defaults into slot 3 under cleared
  flags (save 1), a blank name for slot 2 (save 1), copies of its own read requests
  into unused macro records of slot 1 (save 2), and a new selection (save 2).
- `saves/write-packets.txt`: the 335 profile write packets of those saves as the wire
  carried them, one per line in hex. Two are repeats of a chunk the app sent again.
- `switch-saved.bin`: one save into `switch-erased.bin`. Slot 1 holds the Switch
  defaults, slot 3 the defaults with printed A on X; slot 2 was never opened. The
  vendor app zeroed slot 2's 7 padding bytes; 8B leaves a slot it does not edit as read.
- `swap-on-p2.bin`: a save with slot 3's P2 on Swap.

## Built by 8B and checked on the pad

Each was written over USB, read back byte for byte, and played on the pad.

- `r1.bin`: two remaps of every button, and Auto turbo on P1 and Swap on P2 in slot 3.
- `r2.bin`, `r2b.bin`: stick and trigger ranges, every toggle, and vibration per motor.
- `r3.bin`: macros. Slot 1 holds four: 18 steps on printed A, 10 ms steps on Star, stick
  directions on Home (both sticks on a diagonal in one step), and a 2 s step on P1.
  Slot 2 holds the toggle bits no setting shows (`0x200`, `0x800`, `0x4000`, `0x8000`).
- `r3b.bin`: `r3.bin` with only slot 3's macro changed.
