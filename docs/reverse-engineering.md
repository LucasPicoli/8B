# Reverse engineering a controller

This guide is for a controller the app does not support yet. It takes you from "the pad
is plugged in" to the captures and byte maps that [Adding a controller](adding-a-controller.md)
needs. The commands write the command-line tool as `8b`. Its binary is `8bitdo-pro-3`.
From a source checkout, run `cargo run -p cli --` in its place.

Four rules hold for the whole guide:

1. Read before you write. Replay only packets that you saw the vendor app send while
   it read, never while it wrote, until you know what each byte does. An unknown
   write or erase packet can change or wipe the pad's flash.
2. Before the first write, read the pad's profile memory and save it. That copy is
   your way back.
3. Make the first write a replay of a vendor-app save that you captured, then compare
   the readback with what the app wrote. A write that your own code built comes only
   after that replay matches.
4. Filter a capture to the pad's address before you keep or share it:
   `tshark -r in.pcapng -Y 'usb.device_address == N' -w out.pcapng`. A raw capture
   holds the whole bus, keyboard included, so it never leaves your machine. Keep the
   filtered captures and the dumps. They become the golden fixtures that prove your
   port is correct.

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

## 2. A pad with no config port

Some pads show no config port until they get an unlock sequence. The Ultimate is one:
`dev list` shows no config node, and its vendor app unlocks it, reads and writes, then
relocks it on close. You do not read the app to learn the sequences. You capture them.

1. Capture the vendor app opening and closing normally (section 3 sets up the capture).
   Save the open and the close as two files. The close is the relock.
2. Filter each to the pad and export the rows with `tshark` (section 3). The unlock
   packets go from the host to an interrupt OUT endpoint of the locked pad. The relock
   packets go to the config port after the unlock.
3. Declare both in `description.json` as a `lock`: the locked USB id and product
   string, the unlock interface, endpoint, packets and times, and the relock packets.
   The fields and their checks are in
   [Adding a controller](adding-a-controller.md#a-locked-controller), and the core
   code that reads them is
   [`description/lock.rs`](../crates/controller-core/src/description/lock.rs). The
   Ultimate's own declaration is in
   [`controllers/ultimate/description.json`](../crates/controller-core/controllers/ultimate/description.json).
4. Press "Unlock" in the app. It replays your declaration over usbfs. The access rule
   from the README includes the line that lets you open the pad's usbfs node.
5. Capture that replay on usbmon and compare it with the Windows capture. The bytes
   must match, and each time should match within a millisecond or two. usbfs sends
   what you give it, so the bytes on the wire are the bytes you declared.

Be careful if you send the same packets through a kernel driver instead. A driver can
reshape a packet. xpad, for one, sends the first packet of a new rumble effect twice.
Compare against usbmon, not against what you meant to send.

## 3. Capture the vendor app

The vendor app already speaks the protocol, so a capture of it is the fastest way to
learn it. Run the app in a Windows virtual machine and capture its USB traffic on the
Linux host with Wireshark.

### Set up the capture

1. Load the capture module with `sudo modprobe usbmon`.
2. Find the pad's bus with `lsusb`. A pad on `Bus 008` shows up in Wireshark as the
   `usbmon8` interface.
3. Give yourself read access to that interface with
   `sudo setfacl -m u:$USER:r /dev/usbmon8`. Without it, only root sees the interface.
   The ACL covers one bus and one user, and it ends at the next reboot or module
   reload. Until then it also lets you read the keyboard on that bus.
4. Start the capture, then plug the pad in. Wireshark needs the descriptors the pad
   sends on connect to label its HID traffic.
5. Pass the pad through to the virtual machine by its USB port, not its USB id (see
   below).

Any Windows 10 or 11 guest works. Download the vendor app inside the guest, and check
that its name and version list your pad before you capture anything.

### Pass the pad through by port

A USB id does not follow a pad that comes back with a new product id after a mode
change or an unlock. The port does. Find the pad's bus and port with `lsusb -t`: a pad
on `Bus 008`, port path `2.1`, is `hostbus=8,hostport=2.1`. Hand that to QEMU:

```text
-device usb-host,hostbus=8,hostport=2.1
```

In quickemu, put that option in the VM's `.conf` file as
`extra_args="-device usb-host,hostbus=8,hostport=2.1"`. In libvirt, add it to the
domain XML, with the `qemu` namespace declared on `<domain>`:

```xml
<qemu:commandline>
  <qemu:arg value='-device'/>
  <qemu:arg value='usb-host,hostbus=8,hostport=2.1'/>
</qemu:commandline>
```

QEMU needs to open the pad's node under `/dev/bus/usb`. If it is denied, add a
temporary rule, then reload:

```sh
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="2dc8", TAG+="uaccess"' |
    sudo tee /etc/udev/rules.d/99-vm-pad.rules
sudo udevadm control --reload && sudo udevadm trigger
```

Remove it when the captures are done:
`sudo rm /etc/udev/rules.d/99-vm-pad.rules && sudo udevadm control --reload`.

### Record one action per file

Do one thing in the vendor app, stop the capture and save it. Name the file after the
action, such as `dinput-read.pcapng` or `dinput-vibration-3-to-5.pcapng`. Three small
captures are faster to read than one capture that mixes a read with two changes.

Two habits keep a capture usable:

1. Close the vendor app normally when the action is done. Some pads need what the app
   sends on close (section 2 shows one).
2. Take a screenshot of what you set before each save. The screenshot tells you the
   value to look for in the diff.

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

The first bytes of a request tell you which family the pad belongs to. The middle
column is the value for `framing` in your description:

| A request starts with | `framing` | What it is | Seen on |
| --- | --- | --- | --- |
| `81 04 <command>` | `plain` | The request follows at byte 2 | Pro 3, Ultimate 2 |
| `81 <length> 04 <command>` | `length` | A length byte after the `81` moves the request to byte 3 | Pro 2 (`81 3e 04`), Ultimate (`81 11 04`) |
| `01 66 aa` | `wrapped` | A Nintendo report prefix, used under a Nintendo USB id | Pro 3 in Switch mode |

All three are in
[`protocol/framing.rs`](../crates/controller-core/src/protocol/framing.rs). The length
byte is 17 plus the request's chunk length: `3e` for a 45-byte chunk, `11` for a
command with none. A `length` pad replies in the `plain` layout. The Pro 2 facts come
from [TheJayMann/8bitdo-spec](https://github.com/TheJayMann/8bitdo-spec), whose scripts
read and write a real Pro 2, and the Ultimate 2 facts from
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

## 4. Find the commands

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

## 5. Map the bytes

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

   Each line before the last is one run of changed bytes: the offset, the length, the
   old bytes and the new bytes. The last line counts the runs. When the files differ in
   size, a first line says so and compares only the shorter length. One change in the
   app should move one small run, plus a checksum if the format has one.

Repeat this for each setting, each button and each mode. Record each result as a
named constant in your model's `tables.rs`, as the Pro 3 does in
`crates/controller-core/src/devices/pro3/tables.rs`.

## 6. Hand over to the port

You now have the facts that a port needs:

1. The config ports and model ids, for `description.json`.
2. The command packets, for the driver.
3. The byte map, for the codec.
4. The dumps and captures, for `fixtures/<model>/`. Add a `FIXTURES.md` that says
   where each file came from, as `fixtures/pro3/FIXTURES.md` does.

Continue with [Adding a controller](adding-a-controller.md). If you are stuck, open an
issue with your `dev list --descriptors` output and a capture. A capture holds your
profile names, so rename any profile you would not want to share before you capture.
