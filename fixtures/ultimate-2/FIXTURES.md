# Ultimate 2 fixtures

- `xinput.blob`: the first read of the `XInput` image of an 8BitDo Ultimate 2 Wireless
  (model `0x6012`), firmware 1.10, over its USB cable. 1592 bytes. Slot 1 holds a
  profile named "Crimson" with two macros, L1L2 on L4 and R1R2 on R4. Slots 2 and 3
  are empty.

The unit tests in `crates/controller-core/src/devices/ultimate2/mod.rs` decode it and
encode it back. Every compare skips the CRC at `0x00C`..`0x00F`: the CRC the pad
stores differs from the one the codec computes, and the pad accepts the codec's.
