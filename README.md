# Snapwright

A fast, cross-platform screenshot tool with a real capture library, written in Rust with GPUI.

- Instant: the selection overlay appears within 50 ms of the hotkey.
- Organized: every capture is indexed and searchable, like Snagit's library.
- Everywhere: macOS, Windows and Linux from one codebase.
- Local-first: OCR and redaction run on device.

Status: early development (Phase 0).

## Install

- **macOS:** [Snapwright-macos-universal.dmg](https://github.com/jeremielumandong/Snapwright/releases/latest/download/Snapwright-macos-universal.dmg)
  (Apple silicon and Intel). Grant Screen Recording on first launch.
- **Windows:** [Snapwright-windows-x64-setup.exe](https://github.com/jeremielumandong/Snapwright/releases/latest/download/Snapwright-windows-x64-setup.exe).
- **Linux (x86_64):**

  ```bash
  curl -fsSL https://raw.githubusercontent.com/jeremielumandong/Snapwright/main/install.sh | sh
  ```

  [`install.sh`](install.sh) downloads the latest AppImage, verifies its SHA-256 checksum and
  installs `~/Applications/Snapwright.AppImage`, the `snapwright-app` command, the `snapwright`
  CLI in `~/.local/bin`, and a desktop launcher. No sudo or Rust needed; rerun to update.
  - Pick a release: `curl -fsSL …/install.sh | SNAPWRIGHT_VERSION=v0.2.1 sh`.
  - Replace a running copy: `curl -fsSL …/install.sh | sh -s -- --stop-running`.
  - Uninstall: `curl -fsSL …/install.sh | sh -s -- --uninstall`. Your library in
    `~/.local/share/snapwright` stays.
  - From a checkout: `./install.sh --build` builds the AppImage and installs it;
    `./install.sh dist/Snapwright-<version>-x86_64.AppImage` installs one you have.
