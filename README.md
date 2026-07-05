<div align="center">

<img src="src-tauri/icons/Square310x310Logo.png" alt="Dictara" width="128" height="128">

# Dictara

**Typing is slow. Speaking isn't.**

Free · Bring Your Own Key · Speech-to-Text

Turn your spoken words into text — in any app, any language.

[![Download](https://img.shields.io/badge/Download-Dictara-blue?style=for-the-badge)](https://dictara.app/)

[**Get Dictara**](https://dictara.app/)

</div>

---

## How It Works

1. **Install** — Download and install Dictara
2. **Configure** — Add your OpenAI or Azure OpenAI API key
3. **Dictate** — Hold `FN` to record, release to transcribe. Or press `FN+Space` for hands-free mode
4. **Done** — Text is automatically pasted wherever your cursor is — in any app

---

## Linux (Flatpak)

Dictara ships for Linux as a self-hosted Flatpak, served from our own
repository. Install the latest stable release with one command:

```bash
flatpak install --user https://dictara.app/flatpak/dictara.flatpakref
flatpak run app.dictara.Dictara
```

From then on, `flatpak update` picks up new releases — no self-updater
needed.

Want the bleeding edge? A dev build (`Dictara (Dev)`, app id
`app.dictara.Dictara.Devel`) is rebuilt from every push to `main`. It's a
separate app, so it installs alongside the stable one — with its own
settings — rather than replacing it:

```bash
flatpak install --user https://dictara.app/flatpak/dictara-dev.flatpakref
flatpak run app.dictara.Dictara.Devel
```

The `.flatpak` bundle attached to each
[GitHub Release](https://github.com/vitalii-zinchenko/dictara/releases)
still works too, for offline installs:

```bash
flatpak install --user Dictara_<version>_x86_64.flatpak
```

Wayland is the supported session type. Two things work differently than on macOS:

- **Auto-paste** goes through the XDG RemoteDesktop portal — the first paste
  shows a system permission dialog. The permission is remembered across
  restarts (revoke it under Settings → Apps → Dictara).
  Auto-paste sends Ctrl+V, which works in normal GUI apps; terminal emulators
  use Ctrl+Shift+V instead (there is no universal paste shortcut across GUI
  apps and terminals on Wayland). The transcribed text is always placed on the
  clipboard, so you can paste manually with Ctrl+Shift+V in any terminal.
- **Recording shortcuts** are bound by your desktop environment via the
  GlobalShortcuts portal, not captured in-app. Configure them from the
  system dialog (Settings → Shortcuts in the app). The trigger keys are
  *not* swallowed — they also reach the focused application, so prefer
  combinations you don't use for typing (e.g. `Ctrl+Alt+D`).

GNOME and KDE Plasma implement both portals; wlroots-based compositors
(Sway, Hyprland) may lack GlobalShortcuts support.

To build the Flatpak locally: `scripts/build-flatpak.sh` (uses podman when
the host lacks the webkit2gtk build dependencies).

---

## Troubleshooting

### Emoji Picker Appears When Using Fn Key

If the emoji picker (or character viewer) appears when you press the Fn/Globe (🌐) key, you need to change your macOS keyboard settings:

**Via System Settings (Recommended):**
1. Open **System Settings** → **Keyboard**
2. Find **"Press 🌐 key to"** dropdown
3. Change it to **"Do Nothing"** or **"Change Input Source"**

**Via Terminal:**
```bash
# Set Globe key to "Do Nothing"
defaults write com.apple.HIToolbox AppleFnUsageType -int 0

# Then log out and log back in, or restart your Mac
```

> **Note:** This is a macOS limitation. The Fn/Globe key triggers the emoji picker at a system level that applications cannot intercept.

---

## Contributing

We welcome contributions! Please see our [Contributing Guide](.github/CONTRIBUTING.md) to get started.

## License

This project is licensed under the MIT License — see the [LICENSE](LICENSE) file for details.
