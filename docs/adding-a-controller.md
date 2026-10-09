# Adding a controller

The 8BitDo Pro 3 is the only supported model, so every path below uses it as the
example. No second real model has been ported yet. The tests run a made-up one,
`devices/test_pad.rs`, that differs on purpose: one mode, two slots, no macros and a
64-byte blob. Where the code still assumes the Pro 3, this guide says so (see "What is
tied to the Pro 3 today").

To learn your controller's protocol first, see
[Reverse engineering a controller](reverse-engineering.md).

## What a model is made of

A model has four parts:

1. A controller description: `crates/controller-core/controllers/<model>/description.json`.
   It holds the model's facts: names, USB ids, modes, slot counts, name limits,
   buttons and settings tabs. The format is
   [`schemas/controller-description-v1.schema.json`](../schemas/controller-description-v1.schema.json).
   The loader is
   [`crates/controller-core/src/description.rs`](../crates/controller-core/src/description.rs).
2. The views: one SVG per drawing of the controller, in the same folder. Draw them
   as [Drawing a controller's views](controller-views.md) describes. This guide does
   not repeat it.
3. A `ControllerSpec` implementation. The trait is in
   [`crates/controller-core/src/device.rs`](../crates/controller-core/src/device.rs).
   A model must give its description and its profile blob size. The other methods
   have defaults: the 8BitDo byte values, which only the 8BitDo transport reads, and
   `transport`. The test pad sets only the two required ones and `transport`.
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

A new model gets its own folder next to `pro3/`, and one entry in `MODELS` in
[`devices/mod.rs`](../crates/controller-core/src/devices/mod.rs). Detection, the
transport choice, the keepalive rule and `dev list` read that registry. The udev
access rule does not: a new vendor id needs a line in `UDEV_RULE` (see
[The udev rule](#the-udev-rule)).

For the smallest complete model, read
[`devices/test_pad.rs`](../crates/controller-core/src/devices/test_pad.rs). It is
about 200 lines of code and 100 of tests, and its description sits in
`crates/controller-core/controllers/test-pad/`.

## Modes

The `modes` list of the description names every mode of the model, in display order.
Each entry has an `id`, a `label` and the mode's `extra_outputs`:

```json
{ "id": "xinput", "label": "XInput", "extra_outputs": [] }
```

The `id` is 1 to 15 lowercase letters, digits, `-` or `_`. Profiles, the command
line (`-m xinput`), config ports and settings tabs use it. The app shows the
`label`. A model may have one mode or several, and its ids need not match another
model's. The test pad has a single mode, `standard`.

The codec gets the mode as a `Mode` value. To branch on it, give the driver one
constant per mode, as `devices/pro3/mod.rs` does:

```rust
pub const XINPUT: Mode = Mode::from_static("xinput");
```

A constant can be a `match` pattern. Add a `_` arm that refuses the mode, because
the type holds any id. Test that each constant reads back whole, as
`pro3_mode_constants_match_its_description` does.

## Settings tabs

The `settings` list of the description makes the tabs after Buttons, in the app and
in the patch checks. Each value is named by a JSON pointer into the profile, so a
Sticks tab of the Pro 3 holds `/sticks/left_min_pct`. A tab looks like this:

```json
{
  "id": "vibration",
  "label": "Vibration",
  "lead": "How strongly each motor rumbles. Level 0 turns it off.",
  "frames": [
    {
      "title": "Motors",
      "sliders": [
        { "label": "Left motor", "low": { "field": "/vibration/left_level", "min": 0, "max": 5 } }
      ]
    }
  ]
}
```

A slider with a `high` end edits a range, such as a dead zone. `unit` follows the
value, such as `%`. Without one, the value shows as `3 of 5`. A frame's `flags` are
check boxes, and a flag's `excludes` names the flags that may not be on with it: the
app turns them off, and a patch is refused. A tab with `modes` shows in those modes
only, which is how the Pro 3 has one Triggers tab for analog modes and another for
Switch.

A profile holds each settings group as a JSON object beside its other fields, such as
`"vibration": { "left_level": 5, "right_level": 5 }`. The codec reads and writes those
objects. Validation checks every profile against its model's description: the
`device` and `kind` of the model's default profile, each declared setting present with
a value in range, no undeclared group, the `excludes` rules, and button names. A model
with a stricter JSON schema of its own, as the Pro 3 has in
[`schemas/profile-v1.schema.json`](../schemas/profile-v1.schema.json), returns its
errors from `ProtocolCodec::profile_schema_errors`.

## How a controller is detected

Detection has two stages. Both read the description.

1. The sysfs scan. `scan_sysfs_all` in
   [`detect.rs`](../crates/controller-core/src/detect.rs) lists
   `/sys/bus/usb/devices` and keeps each device whose vendor and product id match a
   `config_ports` entry. The entry also gives the current mode, the USB interface
   number, and the framing.
2. The model check. After the scan, the session sends `START_CONFIG` and reads the
   model id from reply bytes 22 and 23 (`identify` in
   [`transport/session.rs`](../crates/controller-core/src/transport/session.rs)). It
   looks the id up in the registry. If no model lists the id, or the model that
   lists it does not use the config port the pad was found on, the read fails with
   `UnsupportedModel` and nothing else is sent. Every later step of the session uses
   the model it found: blob size, modes, slot count and command bytes.

The second stage matters because one USB id can belong to several models. In XInput
mode the Pro 3 enumerates as `2dc8:310b`, and so does the Ultimate 2 (model id
`0x6012`). The USB id finds a candidate. Only the model id says what it is. List
every model id the model answers with, and the id of no other pad.

Each `config_ports` entry in `description.json` has these fields. One USB id stands for
one mode, so the loader refuses a description that lists an id twice.

| Field | Meaning |
| --- | --- |
| `usb` | `vendor` and `product`, written as `"0x2dc8"` |
| `mode` | The current mode this USB id stands for, one of the ids in `modes` |
| `interface` | USB interface number whose hidraw node carries the config reports |
| `framing` | `plain`, `wrapped` for a Nintendo-style id, or `length` for a pad such as the Pro 2 that puts a length byte after the `81` (see `protocol/framing.rs`) |
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
same way sets the field on its port and needs no change in `udev.rs`, once the model
is in the registry.

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
   `mode_flip_command`, `mode_close_command`. Each refuses a mode the model does not
   have, and their defaults refuse every mode, so a model sets only the ones its
   ports use.
5. The counts in the description: `slot_count` (3 on the Pro 3) and
   `macro_slot_count` (4).

Check the description with the unit test that loads every embedded description. A
bad file fails with the field and the fault.

## A protocol of its own

A model that does not talk the `81 04` config protocol brings its own transport.
Implement [`DeviceIo`](../crates/controller-core/src/transport/device_io.rs) in the
model's folder, and return it from `ControllerSpec::transport`:

```rust
fn transport(&self, port: &str) -> Option<Box<dyn DeviceIo + Send>> {
    Some(Box::new(MyPadDevice::at(port)))
}
```

The app and the command line open every controller with `devices::open`. It finds the
controller's USB id in sysfs, takes the first model whose `config_ports` lists that id,
and uses that model's transport. A model that returns `None`, the default, gets
`HidrawDevice` and the 8BitDo protocol. The test pad returns a `MockDevice`, and
`a_model_with_its_own_transport_gets_it_by_its_usb_id` in `devices/mod.rs` checks
the choice.

`DeviceIo` has 8 methods. The write services call them in the 8BitDo order: slot
select, write the whole blob, apply. A transport without those steps makes slot
select and apply do nothing, and maps the blob of `blob_size` bytes onto its own
reads and writes. The 8BitDo macro and patch commands are methods of `HidrawDevice`
only, so a transport of its own does not implement them.

The app takes the current mode from the USB id of the config port. A pad that changes
mode without changing its USB id lists one port and answers `DeviceIo::current_mode`.
The app asks on every presence poll and prefers that answer, so a mode change shows
and starts a new read.

A pad that talks through HID feature reports can use `get_feature` and `set_feature`
in [`transport/feature.rs`](../crates/controller-core/src/transport/feature.rs) on its
hidraw node. They are the `HIDIOCGFEATURE` and `HIDIOCSFEATURE` ioctls, and the only
`unsafe` code in the workspace, so a driver needs none of its own.

## What is tied to the Pro 3 today

The transport choice, the read and write services and the app pick the model from the
registry. The test pad runs through the services and the app on a mock transport,
never through hidraw. These places still assume the Pro 3:

1. `MacroSlot` in [`model/ids.rs`](../crates/controller-core/src/model/ids.rs)
   accepts 0 to 3, the Pro 3's four macro slots.
2. Two models that share a USB id must also share its interface and framing.
   Detection opens the node of the first model that lists the id before the model id
   says which pad it is.
3. Every request carries a CRC-16/MODBUS, and apply always sends the parameter
   `0x0123`. TheJayMann's Pro 2 scripts send a zero CRC and the parameter `0x15`, so
   a Pro 2 port needs both to come from the model.

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

Each controller in the app keeps the model a read identified. The editor reads that
model's name, modes, buttons, labels, limits, slot count and views from its
description, so a Pro 3 and another model can be plugged in at once.

The command line needs no change either. `set` writes any declared setting by its
pointer, such as `set -m xinput -s 1 /vibration/left_level=3`, and `read-macro` lists
the model's macro slots.
