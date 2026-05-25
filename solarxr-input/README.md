# solarxr-input

An OpenXR binding provider for SlimeVR.

## Available Bindings

| Binding | Description |
|---|---|
| `reset_yaw` | Perform a yaw reset |
| `reset_full` | Perform a full reset |
| `reset_mounting` | Perform a mounting reset |
| `reset_mounting_feet` | Perform a mounting reset for feet |
| `tracking_pause` | Pause tracking |
| `tracking_unpause` | Unpause tracking |
| `tracking_pause_toggle` | Toggle tracking pause |

## Configuration

solarxr-input will read the configuration file located at `~/.config/solarxr-input/config.json`

The default configuration file can be found at `/etc/xdg/solarxr-input/config.json`, or from the source file: [res/config.json](./res/config.json)
