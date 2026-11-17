# Changelog

## 0.3.1 (fork: trihieuvo/fcitx5-osk-custom)

First release of the fork, based on upstream `0.2.1`.

* Renamed to `fcitx5-osk-custom` (binary, D-Bus service, desktop files and packages) so that it can be installed beside, or instead of, the original.
* New `osaka-jade` theme. The store is now loaded even without `--wait-for-socket`, so the configured theme is applied.
* Settings page: grouped into sections, two columns, sliders with proper steps, a restore icon next to values that differ from the restore point, and `Save restore point` / `Restore all` (`restore-point.toml` beside the config file).
* Sizes: `key_height_scale`, `key_spacing_scale` and `key_border_radius`; layout lengths accept decimals (`k1:12.5:1.5`, `p:0.5`).
* Auto show on focus and auto hide on blur (`auto_show_on_focus`, `auto_hide_on_blur`, `auto_hide_delay`, `auto_show_touch_only`), with touch detection through evdev.
* Hold BackSpace, Delete or an arrow key to repeat it.
* Key feedback: press animation, enlarged key preview (the same style for keys with several choices) and synthesized key sounds (`key_animation`, `key_preview`, `key_sound`, `key_sound_volume`).

## Changes Since 0.1.0

The current development version is `0.2.0`. Notable changes since `0.1.0` include:

Thanks to @MorsMortium for giving a lot of feedback.

* Build and packaging now use CMake, with separate install components for `fcitx5-osk` and `fcitx5-osk-kwin-launcher`; Arch packages are available in the AUR.
* The Rust workspace now uses edition 2024, `iced` 0.14, and `iced_layershell` 0.16.
* Manual mode was added, along with the `fcitx5-osk force-show` command for opening the keyboard manually.
* The keyboard UI now supports a quick action bar, reload action, combo mode, repeat key action, custom actions, and popup key feedback while pressing keys.
* Layout and appearance configuration is more flexible: custom themes, auto/light/dark theme selection, XDG config lookup, `.d` asset overrides, global default layout overrides, fractional sizing, and a wider landscape layout are supported.
* Improved lock/login screen behavior through the KWin launcher.
* Reliability fixes include handling `SIGPIPE` without quitting, corrected secondary symbol rendering, better redraw/hover handling, scaling fixes for toggles and drop-downs, and clearer error notifications.
