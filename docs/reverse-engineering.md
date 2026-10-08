# Reverse engineering a controller

This guide is for a controller the app does not support yet. It takes you from "the pad
is plugged in" to the captures and byte maps that [Adding a controller](adding-a-controller.md)
needs. The commands write the command-line tool as `8b`. Its binary is `8bitdo-pro-3`.
From a source checkout, run `cargo run -p cli --` in its place.

Two rules hold for the whole guide:

1. Read before you write. Replay only packets that you saw the vendor app send while
   it read, never while it wrote, until you know what each byte does. An unknown
   write or erase packet can change or wipe the pad's flash.
2. Keep every capture and dump. They become the golden fixtures that prove your port
   is correct.

## 1. Find the config interface

```sh
8b dev list
8b dev list --descriptors
```

`dev list` prints one line per hidraw node, USB or Bluetooth. Each line gives the node,
the bus, the vendor and product id, the USB interface, the report descriptor size and
the name the kernel reports. A node that a supported model uses for config ends in
`[<model> config port, <mode>]`.

A gamepad usually has more than one node. The config node is the one whose report
descriptor declares a vendor-defined usage page (`06 00 ff`, usage page `0xFF00`) with
fixed-size reports. The Pro 3's reports are 64 bytes. To decode a descriptor, install
`hid-tools` and run:

```sh
hid-decode /sys/class/hidraw/hidraw4/device/report_descriptor
```

Flip the pad's mode switch and run `dev list` again. Each mode can have its own USB id
and its own config interface. The Pro 3 has three: `2dc8:310b` on interface 2 for
XInput, `057e:2009` on interface 0 for Switch, and `2dc8:6009` on interface 0 for
DInput. Write them down. They become the `config_ports` of your description.

If `dev list` shows the node but `dev send` says permission denied, install the access
rule from the README, or run the command with `sudo` once.

## 2. Capture the vendor app

The vendor app already speaks the protocol, so a capture of it is the fastest way to
learn it. Run the app in a Windows virtual machine and capture its USB traffic on the
Linux host with Wireshark.

### Set up the capture

1. Load the capture module with `sudo modprobe usbmon`.
2. Find the pad's bus with `lsusb`. A pad on `Bus 008` shows up in Wireshark as the
   `usbmon8` interface.
3. Give yourself read access to that interface with
   `sudo setfacl -m u:$USER:r /dev/usbmon8`. Without it, only root sees the interface.
4. Start the capture, then plug the pad in. Wireshark needs the descriptors the pad
   sends on connect to label its HID traffic.
5. Pass the pad through to the virtual machine. In virt-manager, use Add Hardware, then
   USB Host Device. A pad that changes mode comes back with a new USB id, so pass every
   id that `dev list` showed in section 1. If one is missing, the VM loses the pad in
   the middle of a write.

### Record one action per file

Do one thing in the vendor app, stop the capture and save it. Name the file after the
action, such as `dinput-read.pcapng` or `dinput-vibration-3-to-5.pcapng`. Three small
captures are faster to read than one capture that mixes a read with two changes.

Make these captures first:

1. Connect and read, once in each mode. This gives the hello packet, the read loop and
   a full copy of the pad's profile memory.
2. One setting change and save, for each setting.
3. One button remap and save.
4. A mode change, if the app has one. The pad drops off USB and comes back at a new
   address, so expect two addresses in this file.

### Filter to the config traffic

Wireshark shows every device on the bus. Find the pad's address first. Filter on
`usb.idVendor == 0x2dc8`, and read the address from the Source or Destination column:
`8.7.0` means bus 8, address 7, endpoint 0. Then show only the pad's reports:

```text
usb.device_address == 7 && usb.transfer_type == 0x01 && usb.capdata
```

Transfer type `0x01` is an interrupt transfer, and each row is one HID report. A row
whose source is `host` is a request. A row from the pad is a reply or an input report.

Input reports bury the replies, so filter on the first bytes too. In the plain Pro 3
framing, every request starts with `81 04` and every reply starts with `02 04`:

```text
usb.device_address == 7 && (usb.capdata[0:2] == 81:04 || usb.capdata[0:2] == 02:04)
```

If the filter shows replies but no requests, the app sent its requests as `SET_REPORT`
control transfers. Remove the `usb.transfer_type` term and look for `SET_REPORT` rows.
If no row has `usb.capdata`, your Wireshark version names the field `usbhid.data`. Use
that name in every filter and command in this section.

### Tell the framing apart

The first bytes of a request tell you which family the pad belongs to:

| A request starts with | Framing | Seen on |
| --- | --- | --- |
| `81 04 <command>` | Pro 3 framing, the request follows at byte 2 | Pro 3, Ultimate 2 |
| `81 <length> 04 <command>` | V1 framing, with a length byte before the `04` | Pro 2 (`81 3e 04`) |
| `01 66 aa` | Wrapped Pro 3 framing, used under a Nintendo USB id | Pro 3 in Switch mode |

The Pro 3 framing is in
[`protocol/framing.rs`](../crates/controller-core/src/protocol/framing.rs). The Pro 2
facts come from [TheJayMann/8bitdo-spec](https://github.com/TheJayMann/8bitdo-spec),
and the Ultimate 2 facts from
[ascendedent/8bitdo-Linux-Software](https://github.com/ascendedent/8bitdo-Linux-Software).
If your pad starts its requests with something else, open an issue with the capture.
A new framing is a new `Framing` variant, not a new protocol layer.

### Export for scripts

`tshark` prints the same rows as text, one report per line:

```sh
tshark -r dinput-read.pcapng \
    -Y 'usb.device_address == 7 && usb.capdata' \
    -T fields -e frame.time_relative -e usb.endpoint_address -e usb.capdata
```

An endpoint address of `0x8n` is the pad sending (IN), and `0x0n` is the host sending
(OUT). Keep this text file next to the capture. It is easier to search and to diff
than the capture itself.

## 3. Find the commands

Read the OUT packets of a "connect and read" capture from the top. Look for:

1. A hello packet that the app always sends first. Its reply often carries a model id
   and a firmware version. On the Pro 3 the hello is `81 04 00 01`. Bytes 22 and 23 of
   the reply hold the model id `0x6009`, and bytes 18 and 19 hold the firmware version
   times 100.
2. A read loop: the same command over and over with an offset that grows by a fixed
   step. That is the profile read. The Pro 3 reads a 2348-byte blob in 45-byte
   chunks.
3. A checksum. If two packets differ in a single payload byte and two other bytes also
   change, those two bytes are probably a CRC. The Pro 3 uses CRC-16/MODBUS over the
   payload.

Replay a read-only packet to confirm it:

```sh
8b dev send /dev/hidraw4 81 04 00 01 --pad 64 --match 02
```

`dev send` writes exactly the bytes you give (`--pad 64` fills the rest with zeros),
then prints every report that arrives within `--wait` milliseconds (500 by default).
A pad in a mode that streams input sends a report every few milliseconds. `--match`
shows only the reports that start with the bytes you name. In DInput mode, `--match 02`
hides the Pro 3's input reports (id `04`) and keeps its config replies (id `02`).

While input streams, the pad can drop a config reply. If one is missing, send the
packet again. Many pads have a command that pauses input. The Pro 3's is
`81 04 07 00 00`, and `81 04 07 00 01` resumes it.

## 4. Map the bytes

Change one setting at a time and compare the bytes before and after.

1. Dump the pad's profile memory. Once a driver can read the pad, `8b dump <dir>` saves
   each bank as a raw file. Before that, put the read replies of a capture together
   in order.
2. Change one setting in the vendor app, such as left vibration from 3 to 5, and dump
   again.
3. Compare the two dumps:

   ```sh
   8b dev diff before.blob after.blob
   ```

   Each line is one run of changed bytes: the offset, the length, the old bytes and
   the new bytes. One change in the app should move one small run, plus a checksum
   if the format has one.

Repeat this for each setting, each button and each mode. Record each result as a
named constant in your model's `tables.rs`, as the Pro 3 does in
`crates/controller-core/src/devices/pro3/tables.rs`.

## 5. Hand over to the port

You now have the facts that a port needs:

1. The config ports and model ids, for `description.json`.
2. The command packets, for the driver.
3. The byte map, for the codec.
4. The dumps and captures, for `fixtures/<model>/`. Add a `FIXTURES.md` that says
   where each file came from, as `fixtures/pro3/FIXTURES.md` does.

Continue with [Adding a controller](adding-a-controller.md). If you are stuck, open an
issue with your `dev list --descriptors` output and a capture. A capture holds your
profile names, so rename any profile you would not want to share before you capture.
