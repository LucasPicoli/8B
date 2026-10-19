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
| `edit.rs` | Keep, drop, set-macro and deactivate operations on a blob, and the remap name check |
| `settings.rs` | The sticks, triggers and vibration groups as typed structs, read from and written to the profile's JSON objects |

A new model gets its own folder next to `pro3/`, and one entry in `MODELS` in
[`devices/mod.rs`](../crates/controller-core/src/devices/mod.rs). Detection, the
transport choice, the access rule, the keepalive rule and `dev list` read that
registry (see [The udev rule](#the-udev-rule)).

For the smallest complete model, read
[`devices/test_pad.rs`](../crates/controller-core/src/devices/test_pad.rs). It is
about 190 lines of code and 175 of tests, and its description sits in
`crates/controller-core/controllers/test-pad/`.

## Modes

The `modes` list of the description names every mode of the model, in display order.
Each entry has an `id`, a `label`, a `trigger_kind` and the mode's `extra_outputs`:

```json
{ "id": "xinput", "label": "XInput", "trigger_kind": "analog", "extra_outputs": [] }
```

`trigger_kind` is `analog` when the triggers read a pressure from 0 to 255 in this
mode, and `on_off` when they read pressed or released. A macro step follows it.

A mode where a low stick dead zone stops a macro's stick moves adds
`macro_stick_floor`, the lowest dead zone low end in percent that still plays them
(the Pro 3 has 2 in XInput and Switch). Each stick's dead zone slider then names its
stick with `"macro_stick": "left_stick"` or `"right_stick"`. The app and the command
line warn about a macro that moves a stick whose low end is under the floor, and
still write it. Leave both out when every low end plays.

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

The `settings` list of the description makes the tabs after Buttons in the app, and
gives the range and `excludes` checks that `set` runs. Each value is named by a JSON pointer into the profile, so a
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
value, such as `%`. Without one, the value shows as `3 of 5`. A number that picks one
of a few choices names them in `labels`, one per value from `min` to `max`, and the
app shows the name: `"labels": ["Pulse", "Wave", "Burst"]` with `min` 1 and `max` 3
shows `Wave` for 2. Settings are whole numbers and flags only. A colour, for example,
is three sliders from 0 to 255. A frame's `flags` are
check boxes, and a flag's `excludes` names the flags that may not be on with it: the
app turns them off, and `set` refuses the change. A tab with `modes` shows in those modes
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

## Optional capabilities

A description can say what a model lacks or limits. Each field below is optional, and
leaving it out gives the Pro 3's behaviour. The 8BitDo Ultimate is the example.

### Outputs limited to some buttons

An extra output may list the buttons that take it. The Ultimate offers Turbo on Star,
P1 and P2 only, in both modes, and Screenshot on Star in Switch mode only:

```json
{ "id": "turbo", "label": "Turbo", "buttons": ["star", "p1", "p2"] }
```

Without `buttons`, every remappable button takes the output. Each listed id must be a
button with `can_be_remapped`. An output appears in the modes whose `extra_outputs`
list it, so a Switch-only output sits in the Switch entry alone. The remap check,
profile validation and mode conversion all refuse the output on any other button.

### Codes and indexes in the description

A pad whose profile image stores buttons as numeric codes (the Ultimate) declares
every code in the description, so the codec holds only the image layout. All of these
fields are optional and use hex strings such as `"0x2000"`.

| Field | On | Holds |
| --- | --- | --- |
| `gamepad_mode` | a mode | The byte the image records for the mode and selects it by (3 XInput, 0 Switch). Unique per model. |
| `default_targets` | a mode | What a new profile maps a button to, button id to output id. A button left out sends its own output. The Ultimate maps Star to Turbo in XInput and to Screenshot in Switch, and P1 and P2 to `disabled`. |
| `default_trigger_start` | a mode | Where a new profile starts both triggers' low end, in percent. Absent means 0. The Ultimate's Switch mode has 30. |
| `map_index` | a button | Where its entry sits in the button map (0 to 19). Unique per model. |
| `map_codes` | a button | Its code in a button map entry, one per mode. A mode without a key has no such output, as with Star. The same code can mean another button in another mode: `0x2000` is Xbox A, the bottom face button, in XInput and printed A, the right face button, in Switch. |
| `macro_code` | a button | Its code in a macro record, the same in every mode. Set it on its own. Do not copy `map_codes`, since a model can use different codes in the two places. |
| `code` | an extra output | Its code in a button map entry, such as Turbo `0x10000`. |

A model is one folder with these files and one line in the registry,
`&ultimate::model!("folder")`. Each `Ultimate` value serves the description of its own
folder, so two models never share codes.

A button the macro editor offers needs `macro_code` and `can_start_macro` or
`can_be_in_macro_step`. The codec reads those two flags to know which buttons a record
may hold, so no list of them stays in Rust.

### Firmware gates

A flag, a slider, a number field or an extra output may carry `min_firmware`, the
oldest firmware that has it, written as the pad reports it:

```json
{ "label": "D-pad four-way only", "field": "/dpad/four_way", "min_firmware": "1.03" }
```

`description::firmware_allows(min_firmware, firmware)` decides whether to show the
item for the firmware string of a read. A pad on 1.09 shows a 1.03 item, and a pad
on 1.02 does not. A firmware string that is empty or does not parse hides every gated
item, so an unknown pad never gets a setting it may lack. An item without a gate
always shows.

### Family and validation

`"family_name"` is optional. A model of a family copies the family's name into its
description, such as `8BitDo Ultimate`, and a locked pad shows that name until the unlock
proves which model it is.

`"validated": true` says the model has been checked on a real pad. Leave it out until
then: absent means false, and the app refuses a model that is not validated. Setting
`EIGHTB_UNVALIDATED=1` lets such a model through, and the app then saves a backup file
before every write.

### Profile names

`"profile_rename": false` tells the editor to offer no rename. The Ultimate keeps the
profile names as the pad holds them. `limits.profile_name_length` stays, because the
codec still reads the stored name.

### Macro shape

The Ultimate's macros take one trigger button each, have no name, play once per press
and hold each stick as a direction. Its macro limits, beside `profile_name_length`:

```json
"macro_sticks": "directions8",
"limits": {
  "macro_steps": { "min": 1, "max": 18 },
  "macro_step_ms": { "min": 10, "max": 60990 },
  "triggers_per_macro": 1
}
```

- `triggers_per_macro` caps the trigger buttons of one macro. Leave it out for no
  cap.
- `macro_name_length`, `macro_repeat_count`, `macro_interval_ms` and `glide_step_ms`
  are each optional. Leave one out when the pad lacks that feature: the macro
  editor hides it, and macro validation refuses a file that uses it (a name, a
  repeat count other than 1, a pause other than 0, or a glide).
- `macro_sticks` is `analog` (any x and y from 0 to 255, the default) or
  `directions8` (the centre or one of 8 directions at full deflection). The codec
  maps each direction to its axis values.

### A locked controller

A pad that hides its config port until it gets an unlock sequence declares a `lock`.
The declaration holds every byte and time, so the model needs no unlock code:

```json
"lock": {
  "locked": { "usb": { "vendor": "0x2dc8", "product": "0x3106" },
              "product": "8BitDo Ultimate Controller" },
  "unlock": {
    "interface": 0, "endpoint": "0x02", "timeout_ms": 3000,
    "packets": [ { "at_ms": 0, "data": "0008000100000000" },
                 { "at_ms": 16, "data": "0008000001000000" } ]
  },
  "relock": {
    "packets": [ { "at_ms": 0, "data": "811104070001" },
                 { "at_ms": 8, "data": "8105005102" } ]
  }
}
```

- `locked` is the USB id and the USB product string of the locked pad. Another pad
  may share the USB id, so both must match.
- `unlock` goes to the interrupt OUT `endpoint` of USB `interface` on the locked pad.
  The unlock worked when a config port shows on the same USB port within
  `timeout_ms` of the last packet.
- `relock` goes to the config port as output reports, each zero padded to 64 bytes.
- Each packet is 1 to 64 bytes of hex without spaces. `at_ms` counts from the first
  packet of its sequence and never goes down.

The loader refuses an odd hex length, a time that goes down, an empty sequence, an IN
endpoint, and a locked USB id that is also a config port. A description without a
`lock` always shows its config port.

How 8B acts on it, all in the core:

- `devices::locked_ports()` lists the locked controllers attached, each with its model,
  its USB port path and its usbfs node, `/dev/bus/usb/BBB/DDD`.
- `transport::lock::unlock` opens the usbfs node, takes `interface` from its kernel
  driver, claims it, waits half a second for the pad to take in whatever that driver
  sent last, and writes each packet at its `at_ms`. It keeps the claim while it
  waits for the config port, and gives the interface back to the driver only after.
  A driver given it back at once sends its own start-up packets in the second before
  the pad acts, and the Ultimate then often stays locked. The result is the config
  port's hidraw node, once it opens to read and write, or `Unsupported` with the USB id
  when the port path comes back as an id no description lists.
- `transport::lock::relock` sends `relock` to the config port and waits for the locked
  USB id on the same port path.
- The access rule gains `SUBSYSTEM=="usb", ATTR{idVendor}=="2dc8", TAG+="uaccess"`
  for each vendor with a lock, so the user at the seat may open the usbfs node. A rule
  installed before the line was added reads as outdated, and the app offers to
  install the new one. A denied usbfs node gives the same error as a denied hidraw
  node.

`unlock_and_relock_ten_times` in `tests/ultimate/hardware_lock.rs` runs both on a
locked Ultimate with `just hw`.

## Text and translations

Write the description's text in English only. Translators pick it up from the
`gui.po` of each language under `crates/gui/translations`, so a new model edits no
other file. `cargo run -p controller-core --example description-pot` lists every
string and where it sits, and `just translations` adds new ones to each `gui.po`.

- Write prose in sentence case: page `label` and `lead`, frame `title`, slider and
  flag `label`, value `labels`, `spoken_labels`, view `label`.
- Write names as the pad or its box prints them: `display_name`, `short_name`, `family_name`, mode
  `label`, button `labels` and `printed_label`, `unit`. Every language keeps them as they are.
- Keep each sentence whole inside one `lead`. Never split one sentence across fields.
- Leave out of prose any number the app also shows, such as a range end.

Run `just translations` and commit what it writes: `just lint` fails while `gui.pot`
is older than the description. A string with no translation yet shows in English, and
only warns.

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

A pad that never answers `START_CONFIG` leaves `model_ids` empty. Its config ports
name it, so the session skips `START_CONFIG` and takes the model from the port
(`find_model_by_port` in [`devices/mod.rs`](../crates/controller-core/src/devices/mod.rs)).
No other description may list any of its config port USB ids. If one does, the port
names neither model, and the pad fails with `UnsupportedModel`. The test pad works
this way.

Each `config_ports` entry in `description.json` has these fields. One USB id stands for
one mode, so the loader refuses a description that lists an id twice.

| Field | Meaning |
| --- | --- |
| `usb` | `vendor` and `product`, written as `"0x2dc8"` |
| `mode` | The current mode this USB id stands for, one of the ids in `modes` |
| `interface` | USB interface number whose hidraw node carries the config reports |
| `framing` | `plain`, `wrapped` for a Nintendo-style id, or `length` for a pad such as the Pro 2 that puts a length byte after the `81` (see `protocol/framing.rs`) |
| `write_via` | Optional. A mode to flip to before a write, when this mode takes no writes in place |
| `match_vendor` | Optional, `false` when omitted. `true` if the vendor makes nothing but game controllers, so the access rule may match every hidraw node of the vendor (see the udev rule below) |
| `needs_keepalive` | Optional, `false` when omitted. `true` if the controller resets in this mode while no program holds its event node open (see the keepalive rule below) |

The Pro 3 lists three ports: `2dc8:310b` for XInput (interface 2, plain),
`057e:2009` for Switch (interface 0, wrapped, `write_via` DInput) and `2dc8:6009`
for DInput (interface 0, plain). Only the XInput port sets `needs_keepalive`.

### The udev rule

The app opens the hidraw node of the config interface. The udev rule in
[`transport/udev.rs`](../crates/controller-core/src/transport/udev.rs) gives the
logged-in user access to it.

`access_rule()` writes the rule from the config ports of every description, one line
per vendor or per USB id, each once, in the order the ports are listed:

1. A port with `match_vendor: true` gives a vendor line,
   `KERNEL=="hidraw*", ATTRS{idVendor}=="2dc8"`, which matches every hidraw node of
   the vendor. A new model of that vendor needs no rule change. The Pro 3 sets it on
   its two `2dc8` ports.
2. Any other port gives a line for its USB id. `057e:2009` is one: it is also a
   genuine Nintendo Pro Controller id, and the rule must not open every Nintendo
   device. A per-id port whose vendor another port matches whole adds no line.

The match covers every hidraw node of the device, so a pad that also exposes a
keyboard or mouse interface gets the tag on those nodes too.

An installed access rule is compared to `access_rule()` byte for byte, so when a new
model adds a line, the app asks users to update it. The rule of 0.1.0
(`UDEV_RULE_0_1_0`) counts as current only while `access_rule()` is still the two
access lines it holds.

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
| `framing.rs` | `Plain`, `Wrapped` and `Length` framing of those packets |
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
3. The codec: `map_profile`, `compile_profile`, `default_profile`, the macro
   functions, and the rest of `ProtocolCodec`. The `device` and `kind` of
   `default_profile` are what validation checks every profile against.

   Where the steps live is a codec choice. The Pro 3 keeps a macro's descriptor in
   the blob and its steps on a page of their own, and the default
   `macro_steps_in_blob`, `false`, says so. A model that keeps the steps inside the
   blob returns `true`. Its `decode_macro_metadata` then returns the real steps,
   its `set_macro` writes them, and the services read, write, back up and roll back
   no page: a macro put sends the bank and apply only, and the read-back compares
   the macros decoded from the blob, steps included. The tests in
   `devices/inline_pad.rs` check this on a mock.
4. The `ControllerSpec` values. `apply_argument` is required: the argument the apply
   command (`0x06`) carries, `0x0123` on the Pro 3 and `0x0015` on the Ultimate.
   `firmware_request` and `firmware_version` read the firmware: the request and how
   to read the version from its reply. The request is a `RawRequest`: the report as
   it goes on the wire, which the session sends without its framing, and the first
   two bytes of the reply, which pick the reply out of the other reports. The
   Ultimate sends `81 05 00 21 01` and takes the reply that starts `02 22`. By
   default no request goes out, and the version comes from the `START_CONFIG`
   reply, as on the Pro 3. A model with empty `model_ids` gets no `START_CONFIG`,
   so it sets both, or its firmware shows as unknown. `read_back_writes`, `false` by
   default, reads every profile write back and compares each slot's profile and
   macros, decoded, with what was sent; a difference rolls the write back. The
   Ultimate sets it, because it stores a chunk whose CRC is wrong. The
   per-mode values: `slot_select_value`, `macro_gamepad_mode`,
   `mode_flip_command`, `mode_close_command`. `slot_select_value` and
   `macro_gamepad_mode` refuse a mode the model does not have, and by default they
   refuse every mode. `mode_flip_command` and `mode_close_command` return `None` by
   default. A model sets only the ones its ports use.
5. The counts in the description: `slot_count` (3 on the Pro 3) and
   `macro_slot_count` (4). The macro `limits`: `macro_steps` (1 to 408 on the
   Pro 3), `macro_step_ms` (0 to 65535), `macro_repeat_count` (1 to 99, plus
   continuous), `macro_interval_ms` (0 to 65535) and `glide_step_ms` (20). The
   editor reads them, and a test pins them to the codec, as
   `pro3_macro_limits_match_the_codec` does. A model with no macro slots still lists
   the required ones; the test pad gives each the narrowest range. The optional ones
   are in [Macro shape](#macro-shape).
6. The macro buttons, if the model has macro slots: set `can_start_macro` on each
   button that may start a macro and `can_be_in_macro_step` on each button that may
   appear in a macro step. Both default to `false`, are the same in every mode, and
   `macro_slot_count` above 0 needs at least one button with each. Macro validation
   builds its button lists from them. The Pro 3 has 21 and 16, and a test pins them
   to its codec tables. A button whose press a macro step holds as a trigger value
   (`MacroStep::trigger_left` or `trigger_right`) instead of as a pressed button also
   sets `step_trigger_value` to `"left"` or `"right"`, so the macro editor shows that
   value as the button's press. On the Pro 3 these are L2 and R2.

Check the description with `cargo test -p controller-core every_registered_description_loads`,
which loads the description of every model in `MODELS`. A bad file fails with the
field and the fault.

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

`DeviceIo` has 11 methods. All but `current_mode` are required. The write services
call them in the 8BitDo order: begin the write, slot select, write the whole blob,
apply, end the write. A transport without those steps makes slot select and apply do
nothing, returns `Ok(None)` from `begin_write` and `Ok(())` from `end_write`, and maps
the blob of `blob_size` bytes onto its own reads and writes. A macro put sends the
bank, apply, then `write_macro_stream` (erase the macro's page, then write its steps),
then apply. A model without macro slots never gets the call: the put is refused before any
write. Nor does a model whose steps live in the blob. The
lone 8BitDo erase, the patch write and the status query are methods of `HidrawDevice`
only, so a transport of its own does not implement them.

The app takes the current mode from the USB id of the config port. A pad that changes
mode without changing its USB id lists one port and answers `DeviceIo::current_mode`.
The app asks on every presence poll and prefers that answer, so a mode change shows
and starts a new read.

A pad that talks through HID feature reports can use `get_feature` and `set_feature`
in [`transport/feature.rs`](../crates/controller-core/src/transport/feature.rs) on its
hidraw node. They are the `HIDIOCGFEATURE` and `HIDIOCSFEATURE` ioctls. They and
the usbfs ioctls in
[`transport/usbfs.rs`](../crates/controller-core/src/transport/usbfs.rs) are the only
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
3. Every request carries a CRC-16/MODBUS. TheJayMann's Pro 2 scripts send a zero
   CRC, so a Pro 2 port needs it to come from the model.
4. Before the first read, the app shows the first model in `MODELS`, so the Pro 3
   stays first in the registry.
5. The command-line tool is named `8bitdo-pro-3`, and its help text names the Pro 3.

## Prove the port is correct

A port is correct when the bytes match, not when the code looks right.

1. Capture real blobs. Read each bank from a controller and save it. Save a write
   made by the vendor's app too, if you can capture one.
2. Put them in `fixtures/<model>/`, next to their decoded JSON. The Pro 3 catalog
   [`fixtures/pro3/FIXTURES.md`](../fixtures/pro3/FIXTURES.md) says where each file
   came from and how to regenerate the ones the encoders make.
3. Write golden-vector tests as the Pro 3 does in
   [`crates/controller-core/tests/pro3/`](../crates/controller-core/tests/pro3/).
   Each model keeps its integration tests in `tests/<model>/`, entered from that
   folder's `main.rs`, which lists the files as modules. Cargo builds one test binary
   per model, run with `cargo test -p controller-core --test <model>`, so one model's
   tests never mix with another's. In the Pro 3's folder,
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

   They are `#[ignore]` tests behind the `hardware` feature. The Pro 3's are
   `hardware_read.rs`, `hardware_write.rs`, `hardware_macro.rs` and
   `hardware_batch.rs` in `tests/pro3/`. They read every bank, write a remap and
   revert it, write and remove a macro, and time batch writes.

   Every model shares the guard in
   [`tests/support/hw.rs`](../crates/controller-core/tests/support/hw.rs):
   - `hw::pad(&Pro3)` finds an attached pad that answers as that model, and panics
     before any write when there is none. A test never writes one model's blobs to
     another pad.
   - `hw::seed(&dev, &Pro3, fill)` calls `fill(mode, bank)` for each bank, writes the
     bytes it returns, and returns a guard that puts the banks back byte for byte when
     the test ends, pass or fail.

   A model's `main.rs` pulls the guard in with
   `#[cfg(feature = "hardware")] #[path = "../support/hw.rs"] mod hw;`, as
   `tests/pro3/main.rs` does.

   The Pro 3's `seed.rs` fills every slot with the fixture profiles, so its tests run
   on any Pro 3, even one with every slot empty. A new model writes its own `seed.rs`
   with its own fixtures. The tests write to the pad's flash: keep a dump of its banks
   anyway.
6. Run `just lint` before you commit.

## What the app needs

Each controller in the app keeps the model a read identified. The editor reads that
model's name, modes, buttons, labels, limits, slot count and views from its
description, so a Pro 3 and another model can be plugged in at once.

The command line needs no change either. `set` writes any declared setting by its
pointer, such as `set -m xinput -s 1 /vibration/left_level=3`, and `read-macro` lists
the model's macro slots.
