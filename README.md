<div align="center">

<img src="packaging/linux/io.github.LucasPicoli._8B.svg" alt="8B app icon" width="128" height="128">

# 8B

**Read and edit the profiles stored on your 8BitDo controller, over USB, on Linux.**

[Download](#download) · [Supported devices](#supported-devices) · [USB permission](#usb-permission) · [Build from source](#build-from-source) · [Contributing](#contributing) · [Licence](#licence)

![8B editing a DInput profile: saved remaps in blue, unsaved edits in orange](docs/images/screenshot.png)

</div>

## Supported devices

| Controller | Supported features |
| --- | --- |
| 8BitDo Pro 3 | Edit button mapping, sticks, triggers and vibration in every slot of XInput, Switch and DInput. Clear slots. Import and export profiles as JSON. See which button starts each macro. Macro steps cannot be viewed or edited yet, and remapping a macro's button removes the macro. |

To add another controller, start with [Reverse engineering a controller](docs/reverse-engineering.md),
then see [Adding a controller](docs/adding-a-controller.md).

## Roadmap

- [ ] Macro creation and editing
- [ ] Support for more controllers
- [ ] Internationalization (translated interface)

## Download

Download the latest AppImage from the [releases page](https://github.com/LucasPicoli/8B/releases), run `chmod +x` on it, and start it. x86_64 is tested; the aarch64 build has not been tested on hardware.

The same page has a Flatpak bundle for each architecture, `8B-<version>-<arch>.flatpak`, with a `.sha256` file. Install it with `flatpak install --user <file>`.

## USB permission

Linux blocks apps from opening the controller until a device rule allows it. When 8B is denied access to the controller, it asks to install one and prompts for your password once (through `pkexec`). It writes `/etc/udev/rules.d/70-8b.rules`, which gives the logged-in user access to 8BitDo controllers and to USB id `057e:2009` (the Pro 3 in Switch mode, an id it shares with the Nintendo Pro Controller), then reloads udev. If you prefer, the window shows the same steps as one command to run yourself. The Flatpak cannot install the rule, so it shows only the command.

To remove it:

```sh
sudo rm /etc/udev/rules.d/70-8b.rules
sudo udevadm control --reload-rules
```

If you also installed the keepalive fix ("Keep the controller connected"), remove its files too, before you reload the rules:

```sh
sudo rm /etc/udev/rules.d/71-8b-keepalive.rules /etc/systemd/system/8b-keepalive@.service
sudo systemctl daemon-reload
```

Then unplug the controller and plug it back in. 8B asks again the next time it is denied access.

## Build from source

The toolchain is pinned in `rust-toolchain.toml`; `rustup` installs it on the first build.

```sh
cargo run -p gui     # run the app
just appimage        # build the x86_64 AppImage in a ubuntu:22.04 container
just flatpak         # build the Flatpak bundle into packaging/flatpak/out/
```

`just appimage` needs [`just`](https://github.com/casey/just) (`cargo install just`, or your distro's package) and either `podman` or `docker`. `just flatpak` needs `flatpak-builder`, `uv` and `jq`.

A command-line tool, `8bitdo-pro-3`, lives in `crates/cli`.

## Contributing

Bug reports, fixes and new controllers are welcome. To add a controller, start with
[Reverse engineering a controller](docs/reverse-engineering.md), then follow
[Adding a controller](docs/adding-a-controller.md) and
[Drawing a controller's views](docs/controller-views.md).

1. Install `just` and the fontconfig headers (`libfontconfig-dev` on Debian and Ubuntu).
2. Make your change on a branch.
3. Run `just lint`. It runs `cargo fmt --check`, clippy with warnings as errors, and the
   tests. CI runs fmt, clippy and `cargo test` on every push and pull request.
4. If you changed the protocol, an encoder or a write path, run `just hw` with the
   controller attached. These tests write to the pad's flash and put every bank back
   when they end, pass or fail. Keep a dump of your banks anyway.
5. Open a pull request. Commit subjects start with `feat:`, `fix:`, `refactor:`,
   `test:` or `docs:`.

The code follows these rules:

- Library code does not panic. Return a `Result`, and never call `unwrap` or
  `expect` or index a raw slice. The bounds-checked accessors in `protocol::bytes`
  read and write blob bytes.
- Every public item has a doc comment. A fallible function documents its errors in
  an `# Errors` section.
- Offsets and sizes are named, documented constants, never bare numbers.
- `unsafe` is denied in the whole workspace. The one exception is the HID
  feature-report ioctls in `transport/feature.rs`, and each block there has a
  `// SAFETY:` comment.
- Captured blobs are the oracle. When a golden-vector test and the code disagree,
  the code is wrong.
- In the GUI, place anything centred or stretched with `1phx` rounding, never `1px`,
  and build every dialog on `DialogShell`. A fractional physical-pixel offset blurs
  text.

Contributions are licensed under GPL-3.0-or-later, the same as the project.

## Licence

8B is licensed under GPL-3.0-or-later. See [LICENSE](LICENSE).

> [!NOTE]
> 8B is an independent project, not made or endorsed by 8BitDo.
