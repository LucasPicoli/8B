# Ultimate 2 fixtures

- `xinput.blob`: the first read of the `XInput` image of an 8BitDo Ultimate 2 Wireless
  (model `0x6012`), firmware 1.10, over its USB cable. 1592 bytes. Slot 1 holds a
  profile named "Crimson" with two macros, L1L2 on L4 and R1R2 on R4. Slots 2 and 3
  are empty.

The unit tests in `crates/controller-core/src/devices/ultimate2/mod.rs` decode it and
encode it back. Every compare skips the CRC at `0x00C`..`0x00F`: the CRC the pad
stores differs from the one the codec computes, and the pad accepts the codec's.

- `xinput-vendor-triggers.steps.bin`: the seven step records (70 bytes) the vendor app wrote
  to an Ultimate 2 in XInput, firmware 1.10, captured on usbmon and read from the macro
  write packets at wire byte 18. The macro is LT down, LT up, RT down, RT up, both down,
  LT up, RT up, 500 ms apart. The app stores L2 as `keys` `0x4000` and R2 as `0x8000`,
  `trigger_value` 0, sticks `7f7f`. The 26 bytes it sent after the last record are left
  out: they are stale data from its buffer.

- `erase-probe/dinput-slot3.json`: the `DInput` slot 3 profile the erase probes in
  `crates/controller-core/tests/ultimate2/erase_probe.rs` write. It keeps a macro in
  macro slot 2, the page the firmware 1.10 erases, which the GUI check's own file now
  leaves empty.
