# Language tray indicator

Small Rust tray app for **niri on Wayland**. Uses the layouts already configured in niri, not the system locale. Requires a StatusNotifier tray host, such as Waybar with its `tray` module enabled. Install `zenity` for the shortcut dialog, or use the CLI instead.

```sh
cargo run --release
```

- The flag follows layout changes, including changes made outside the app and per-window layouts.
- Left-click cycles languages. Right-click selects a language or configures the shortcut.
- The custom shortcut is disabled by default. Your existing system shortcuts are unchanged.

Flags are included for US and UK English, Polish, German, French, Italian, Russian, and Ukrainian. Other layouts show a keyboard icon with the full layout name in the tooltip.

## Optional shortcut

1. Start the app once to create its config files.
2. Add this line once to `~/.config/niri/config.kdl`, using your actual absolute home path:

   ```kdl
   include "/home/YOUR_USER/.config/language-tray-indicator/shortcut.kdl"
   ```

   If you set `XDG_CONFIG_HOME`, use that directory instead of `~/.config`. The shortcut dialog and CLI print the exact include line.

3. Right-click the tray, choose **Configure shortcut...**, enter a binding such as `Ctrl+Alt+L`, then check **Enable shortcut**.

Niri handles the global shortcut and reloads the include automatically. Uncheck it to disable it. Settings survive app restarts. The native binding also works while the tray is closed. Choose a key not already bound in niri, and avoid duplicating an XKB layout-switch shortcut.

CLI setup, before starting the tray:

```sh
cargo run --release -- --shortcut 'Ctrl+Alt+L'  # Sets the key, does not enable a disabled shortcut.
cargo run --release -- --enable-shortcut
cargo run --release -- --disable-shortcut
```

The app manages `settings.json`, `shortcut.kdl`, and `validate.kdl` under its config directory. It never edits your niri config or keyboard layouts. Shortcuts support `Mod`, `Super`, `Ctrl`, `Alt`, and `Shift` plus an XKB key name. Niri validates each binding before it is saved.

## Checks

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```
