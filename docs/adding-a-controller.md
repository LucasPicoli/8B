# Adding a controller

The 8BitDo Pro 3 is the only supported model, so every path below uses it as the
example. Nothing here has been tested against a second model. Where the code still
assumes the Pro 3, this guide says so (see "What is tied to the Pro 3 today").

To learn your controller's protocol first, see
[Reverse engineering a controller](reverse-engineering.md).

## What a model is made of

A model has four parts:

1. A controller description: `crates/controller-core/controllers/<model>/description.json`.
   It holds the model's facts: names, USB ids, modes, slot counts, value limits and
   buttons. The format is
   [`schemas/controller-description-v1.schema.json`](../schemas/controller-description-v1.schema.json).
   The loader is
   [`crates/controller-core/src/description.rs`](../crates/controller-core/src/description.rs).
2. The views: one SVG per drawing of the controller, in the same folder. Draw them
   as [Drawing a controller's views](controller-views.md) describes. This guide does
   not repeat it.
3. A `ControllerSpec` implementation: the protocol bytes that are not data. The
   trait is in
   [`crates/controller-core/src/device.rs`](../crates/controller-core/src/device.rs).
   It gives the profile blob size, the slot-select value and macro gamepad byte of
   each mode, and the mode-flip and mode-close packets.
4. A `ProtocolCodec` implementation, in the same file: the codec that turns a raw
   blob into a canonical profile and back, and does the same for macros.

The Pro 3 lives in
[`crates/controller-core/src/devices/pro3/`](../crates/controller-core/src/devices/pro3/):

| File | Holds |
| --- | --- |
| `mod.rs` | The `Pro3` type, both trait implementations, and the embedded description |
| `tables.rs` | Blob offsets, strides, and the 4-byte button encodings |
| `profile.rs` | `map_profile` (decode) and `compile_profile` (encode) |
| `macros.rs` | Macro metadata and step encode and decode |
| `edit.rs` | Keep, drop and deactivate operations on a blob |

A new model gets its own folder next to `pro3/`, and one line in
[`devices/mod.rs`](../crates/controller-core/src/devices/mod.rs).

## How a controller is detected

Detection has two stages. Both read the description.

1. The sysfs scan. `scan_sysfs_all` in
   [`detect.rs`](../crates/controller-core/src/detect.rs) lists
   `/sys/bus/usb/devices` and keeps each device whose vendor and product id match a
   `config_ports` entry. The entry also gives the current mode, the USB interface
   number, and the framing.
2. The model check. After the scan, the session sends `START_CONFIG` and reads the
   model id from reply bytes 22 and 23 (`identify` in
   [`transport/session.rs`](../crates/controller-core/src/transport/session.rs)).
   If the id is not in the description's `model_ids`, the read fails with
   `UnsupportedModel` and nothing else is sent.

The second stage matters because one USB id can belong to several models. In XInput
mode the Pro 3 enumerates as `2dc8:310b`, and so does the Ultimate 2 (model id
`0x6012`). The USB id finds a candidate. Only the model id says what it is. List
every model id the model answers with, and the id of no other pad.

Each `config_ports` entry in `description.json` has these fields:

| Field | Meaning |
| --- | --- |
| `usb` | `vendor` and `product`, written as `"0x2dc8"` |
| `mode` | The current mode this USB id stands for: `xinput`, `switch` or `dinput` |
| `interface` | USB interface number whose hidraw node carries the config reports |
| `framing` | `plain`, or `wrapped` for a Nintendo-style id (see `protocol/framing.rs`) |
| `write_via` | Optional. A mode to flip to before a write, when this mode takes no writes in place |
| `needs_keepalive` | Optional, `false` when omitted. `true` if the controller resets in this mode while no program holds its event node open (see the keepalive rule below) |

The Pro 3 lists three ports: `2dc8:310b` for XInput (interface 2, plain),
`057e:2009` for Switch (interface 0, wrapped, `write_via` DInput) and `2dc8:6009`
for DInput (interface 0, plain). Only the XInput port sets `needs_keepalive`.

### The udev rule

The app opens the hidraw node of the config interface. The udev rule in
[`transport/udev.rs`](../crates/controller-core/src/transport/udev.rs) gives the
logged-in user access to it:

1. `KERNEL=="hidraw*", ATTRS{idVendor}=="2dc8"` matches every hidraw node of vendor
   `2dc8` (8BitDo). A new 8BitDo model needs no change.
2. `057e:2009` has its own line, because it is also a genuine Nintendo Pro Controller
   id and the rule must not open every Nintendo device.
3. A model with a new vendor id needs a new line in `UDEV_RULE`. Match the vendor
   and, if the vendor makes other devices, the product id.

An installed access rule is compared to `UDEV_RULE` byte for byte, so after a change
the app asks users to update it. The rule of 0.1.0 (`UDEV_RULE_0_1_0`) also counts as
current.

### The keepalive rule

Some pads reset when nothing polls them. The Pro 3 in XInput mode reconnects every
1.6 s until a program opens its event node, because `xpad` polls a wired pad only
while that node is open. That fix is a separate install from the access rule:
`71-8b-keepalive.rules` plus the `8b-keepalive@.service` unit, with their own command
(`keepalive_command`).

`keepalive_rule()` writes one line per config port with `needs_keepalive: true`,
matching the `xpad` event node by vendor and product id. A new model that resets the
same way sets the field on its port and needs no change in `udev.rs`, once its
description is listed in `descriptions()` there.

## The protocol layer a model can reuse

[`crates/controller-core/src/protocol/`](../crates/controller-core/src/protocol/)
has no device state and no I/O:

| File | Holds |
| --- | --- |
| `wire.rs` | 64-byte request builders (`START_CONFIG`, slot select, profile upload) and the upload reply decoder |
| `wire_write.rs` | Write, apply and macro erase and write packets, and their reply checks |
| `framing.rs` | `Plain` and `Wrapped` framing of those packets |
| `crc16.rs` | CRC-16/MODBUS, which covers each request's payload |
| `bytes.rs` | Bounds-checked read and write accessors. Use these, never raw indexing |
| `text.rs` | UTF-16BE profile and macro names |

If the new model talks the same `81 04` config protocol, it reuses all of this. The
profile is read and written in 45-byte chunks starting at payload offset 18.

The model supplies:

1. The blob layout, in a `tables.rs` of its own. Name every offset, stride and size.
   For the Pro 3 that is a 2348-byte (`0x092C`) blob with three profile slots, a
   button map at `0x00E4` with a stride of `0x5C` per slot, and 22 source buttons
   with 4 bytes each.
2. The button map: one 4-byte encoding per output, per mode, and the order of the
   source buttons. The `id` of each button in `description.json` is the name the
   codec uses in `button_mappings`.
3. The codec: `map_profile`, `compile_profile`, the macro functions, and the rest of
   `ProtocolCodec`.
4. The per-mode `ControllerSpec` values: `slot_select_value`, `macro_gamepad_mode`,
   `mode_flip_command`, `mode_close_command`.
5. The counts in the description: `slot_count` (3 on the Pro 3) and
   `macro_slot_count` (4).

Check the description with the unit test that loads every embedded description. A
bad file fails with the field and the fault.

## What is tied to the Pro 3 today

The description and the codec are per model. The code around them is not yet. A
second model has to change these places, or remove the assumption first:

1. The transport names `Pro3` directly. `Target.spec` in `transport/session.rs`,
   `HidrawDevice.spec` in `transport/hidraw_device.rs` and the erase path in
   `transport/hidraw_write.rs` hold a `Pro3`, not a `ControllerSpec`. There is no
   registry that maps a detected model id to a spec.
2. The blob size is a constant in four more places: `PROFILE_SIZE` in
   `transport/write_input.rs`, `PROFILE_SIZE` in `detect.rs`, `PROFILE_SIG` in
   `protocol/wire.rs` and `protocol/wire_write.rs`. A model with another blob size
   needs them to come from `ControllerSpec::blob_size`.
3. `Slot` accepts 1 to 3 and `MacroSlot` 0 to 3 (`model/ids.rs`), whatever the
   description says. `Mode` is a fixed enum of `XInput`, `Switch` and `DInput`.
4. `service/validation/macros.rs` and `orchestrator/patch.rs` read
   `devices::pro3::tables` directly, and the profile model in
   [`model/profile.rs`](../crates/controller-core/src/model/profile.rs) has fixed
   stick, trigger and vibration fields. `kind` and `device` carry Pro 3 strings.
5. The command-line tool and the app call `Pro3` where they need a spec
   (`crates/cli`, and `crates/gui/src/main.rs`, `writes.rs`, `review.rs` and
   `buttons.rs`).

A first port therefore starts with items 1 to 4, in a change of their own, before
the model folder. The Pro 3 golden tests must stay green through it.

## Prove the port is correct

A port is correct when the bytes match, not when the code looks right.

1. Capture real blobs. Read each bank from a controller and save it. Save a write
   made by the vendor's app too, if you can capture one.
2. Put them in `fixtures/<model>/`, next to their decoded JSON. The Pro 3 catalog
   [`fixtures/pro3/FIXTURES.md`](../fixtures/pro3/FIXTURES.md) says where each file
   came from and how to regenerate the ones the encoders make.
3. Write golden-vector tests as the Pro 3 does in
   [`crates/controller-core/tests/`](../crates/controller-core/tests/):
   `golden_profile_decode.rs` decodes a blob and compares it to the JSON,
   `golden_profile_compile.rs` compiles a profile and compares every byte to the blob,
   and `golden_macro_decode.rs` and `golden_macro_encode.rs` do the same for macros.
   The fixtures are the oracle. When code and fixture disagree, the code is wrong.
4. Run the tests:

   ```sh
   cargo test -p controller-core
   ```

5. Run the hardware tests with the controller attached:

   ```sh
   just hw
   ```

   They are `#[ignore]` tests behind the `hardware` feature
   (`tests/hardware_read.rs`, `hardware_write.rs`, `hardware_macro.rs`). They read
   every bank, write a remap and revert it, and write and remove a macro. They
   assume a Pro 3, so a new model needs its own copies. They write to the pad: keep
   a backup of its profiles first.
6. Run `just lint` before you commit.

## What the app needs

The editor reads the model's name, modes, buttons, labels, limits, slot count and
views from the controller description, so those need no app change. The places in
"What is tied to the Pro 3 today" that touch `crates/gui` and `crates/cli` still
name the Pro 3. Until they are fixed, a second model needs a change there too.
