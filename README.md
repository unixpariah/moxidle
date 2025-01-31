# Moxidle

Feature-rich Wayland idle daemon.

## Features

- Implements the `ext-idle-notify-v1` Wayland protocol
- Supports `loginctl` commands (lock, unlock, before-sleep)
- Handles DBus idle-inhibit (used by applications like Firefox and Steam)
- Supports audio-based idle inhibition
- Allows for configurable conditional timeouts

## Configuration

Moxidle's configuration is written in Lua and is located at `$XDG_CONFIG_HOME/moxidle/config.lua` or `~/.config/moxidle/config.lua`. The format is heavily inspired by hypridle

### Example Configuration

```lua
return {
  general = {
    lock_cmd = "pidof hyprlock || hyprlock", -- Command to lock the session
    unlock_cmd = "notify-send 'Unlocking'", -- Command to run on unlock
    before_sleep_cmd = "notify-send 'Going to sleep'", -- Command executed before sleep
    after_sleep_cmd = "notify-send 'Awake!'", -- Command executed after waking up
    ignore_dbus_inhibit = false, -- Ignore DBus idle-inhibit requests
    ignore_systemd_inhibit = false, -- Ignore systemd idle inhibitors
    ignore_audio_inhibit = false, -- Ignore audio activity inhibition
  },
  timeouts = {
    {
      timeout = 300, -- Idle timeout in seconds
      on_timeout = "pidof hyprlock || hyprlock", -- Command executed on timeout
      on_resume = "notify-send 'Welcome back!'", -- Command executed on user activity
    },
  },
}
```

You can define multiple timeout rules. If `on_timeout` or `on_resume` is omitted, those events will be ignored.

## Buildtime dependencies

- wayland-protocols
- Lua 5.4
- libpulseaudio (Optional if disabling default features)
- Rust 

## Runtime dependencies

- dbus (Optional if disabling default features)
- upower (Optional)

## Building

```sh
cargo build --release
```

If you wish to disable dbus/systemd/audio/upower integration use and enable necessary ones with individual feature flags

```sh
cargo build --no-default-features
```

### Feature flags

- dbus - Enables dbus integration
- systemd - Enables Systemd integration (enables dbus)
- upower - Enables integration with upower dbus service (enables dbus)
- audio - Enables audio integration

## Installation

```sh
cargo install --path .
```

## Usage

To start Moxidle manually:

```sh
moxidle
```

For automatic startup, you can add it to your compositor's config or use systemd:

```sh
systemctl --user enable --now moxidle.service
```

## Command-line Flags

```
-c <config_path>, --config <config_path>  Specify a custom config path (default: ~/.config/moxidle/config.lua)
-q, --quiet                               Suppress output
-v, --verbose                             Enable verbose logging
```
