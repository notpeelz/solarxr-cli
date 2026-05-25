# solarxr-cli

A command-line tool for interfacing with the [SlimeVR server](https://github.com/SlimeVR/SlimeVR-Server) via the [SolarXR protocol](https://github.com/SlimeVR/SolarXR-Protocol).

Also includes [solarxr-input](./solarxr-input), an OpenXR binding provider for SlimeVR.

---

## Installation

### Arch Linux (AUR)

[solarxr-cli-git](https://aur.archlinux.org/packages/solarxr-cli-git)

### From Source

```sh
git clone --recurse-submodules https://github.com/notpeelz/solarxr-cli.git
cd solarxr-cli
cargo make install out -- --release
./out/usr/bin/solarxr-cli --help
```

---

## Usage

```sh
# Perform a yaw reset
solarxr-cli reset yaw

# Perform a yaw reset and display a notification when done
solarxr-cli reset --delay 3 --wait yaw && notify-send -e "Yaw Reset"

# Toggle tracking pause
solarxr-cli tracking pause toggle
```

---

## License

This project is licensed under the [GNU General Public License v3.0](https://www.gnu.org/licenses/gpl-3.0.html).

---

## Related Projects

- [SlimeVR Server](https://github.com/SlimeVR/SlimeVR-Server) - The SlimeVR server this tool interfaces with
- [SolarXR Protocol](https://github.com/SlimeVR/SolarXR-Protocol) - The underlying protocol
