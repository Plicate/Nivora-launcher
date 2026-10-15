<div align="center">

# Nivora Launcher

**A universal Minecraft launcher: Java, Bedrock and servers — in one window**

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-blue)]()
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-green)]()
[![Tauri 2](https://img.shields.io/badge/Tauri-2-orange)]()
[![Version](https://img.shields.io/badge/version-1.0.0-purple)]()

**[English](README.md)** | **[Русский](README.ru.md)**

A modern Vue 3 interface, a fast Rust core, and an honest approach to
licenses and your data. macOS is not supported — no pretending otherwise.

</div>

---

## About

Nivora launches the **Java Edition of Minecraft** (vanilla, Fabric, Quilt,
Forge, NeoForge), **server cores** (Paper, Purpur, Pufferfish, Folia,
Spigot, Leaf, Patina) and **Minecraft Bedrock** — on Linux through the
embedded BedrockOnLinux runtime, on Windows through the official
“Minecraft for Windows” from the Microsoft Store. No game repacks: Bedrock
is always installed from official sources under your account’s license.

- **Instances instead of a single install** — isolated folders, groups,
  tags, icons, playtime, duplication, ZIP export.
- **Catalogs** — mods, resource packs, shaders, data packs and modpacks
  from Modrinth, CurseForge, FTB and Technic; server plugins from
  Modrinth, Hangar, Spiget and GitHub Releases.
- **Microsoft accounts** — device-code sign-in, Xbox Live/XSTS, skins and
  capes, multiple accounts with aliases. Passwords are never stored.
- **In-game overlay** (`Shift+Tab`) — mods, session time, system metrics
  and video recording (`F7`) on top of the game.
- **Workspace** — live console, snapshots, screenshot gallery, file
  manager, CPU/RAM/disk metrics.
- **Personalization** — a theme center in the spirit of Blender/Unity:
  colors, shapes, wallpaper, fonts, layout, animations; themes from a
  GitHub catalog.
- **Self-update** — check for and install updates right from the launcher
  (Settings → Self-update); the feed is `latest.json` from GitHub
  Releases, packages are signed with a minisign key. On Linux
  self-update works for AppImage; deb/rpm are updated by reinstalling
  the package.
- **More** — WebDAV data sync, HTTP/SOCKS5 proxy, settings profiles,
  plugin permission limits without code execution, CLI and
  `doctor` diagnostics.

The full feature list lives in [docs/FEATURES.ru.md](docs/FEATURES.ru.md)
(Russian).

## Screenshots

> Put screenshots into `docs/screenshots/` and uncomment:
>
> | Library | Overlay | Theme studio |
> |:---:|:---:|:---:|
> | ![Library](docs/screenshots/library.png) | ![Overlay](docs/screenshots/overlay.png) | ![Themes](docs/screenshots/themes.png) |

## Installation

| OS | Method |
|---|---|
| **Windows 10/11** | `.exe` installer (NSIS). WebView2 is required (installed automatically). Bedrock — the official “Minecraft for Windows” from the Microsoft Store: the launcher offers installation via winget or the Store page. |
| **Debian/Ubuntu** | `.deb` package (`python3` is pulled in as a dependency). |
| **Fedora/RHEL** | `.rpm` package. |
| **Arch/CachyOS** | Build the package with `scripts/build_arch_package.sh`. |

Ready-made packages are published on the Releases page; the launcher
checks for new versions on its own.

> **Windows SmartScreen:** the installer is not signed with an EV
> certificate, so Windows may warn “Windows protected your PC” on first
> run — “More info” → “Run anyway”. This is normal for unsigned
> open-source builds; the launcher itself verifies the integrity of
> everything it downloads.

> **Linux + Wayland:** an overlay window cannot be drawn on top of another
> app’s window on Wayland — the overlay opens inside the launcher window
> (a button in the top bar, a bound hotkey, or the tray item). On X11 it
> works on top of the game.

## Building from source

```bash
git clone <repo> nivora-launcher && cd nivora-launcher

# Windows (from WSL/a Linux cross environment, or from Windows)
bash scripts/build_windows_exe.sh

# Linux
cd frontend && npm install && npm run tauri build   # .deb/.rpm/AppImage

# Development
cd frontend && npm install && npm run tauri dev
```

The Python backend requires Python 3.10+. Pre-release checks:

```bash
cargo test --locked                                # Rust core
python3 -m unittest discover -s tests -q           # Python side
cd frontend && npm run build                       # vue-tsc + vite
```

## Minecraft Bedrock

- **Windows** — only the official “Minecraft for Windows” from the
  Microsoft Store. Installed via winget with a fallback to the Store
  page, launched through the `minecraft://` protocol; content lives in
  the package’s `com.mojang` folder.
- **Linux** — the embedded BedrockOnLinux **2.2.9** (SHA-256 pinned in
  the sources, integrity is verified on every launch). The game is
  downloaded from the Microsoft Store CDN **under your license**: the
  account signed in to Nivora must own Minecraft.
- The Microsoft sign-in is **shared by all Bedrock instances** (a single
  xodus license store) — no extra device registrations are created.
- Without a sign-in the game will not start: its files are encrypted
  with the license of the owning account. The “Microsoft Store license”
  card in the workspace always shows the status.
- The Bedrock workspace supports a session console with a log archive,
  in-game screenshots, content snapshots (packs/add-ons/worlds) and a
  profile file manager.

## Architecture

```
frontend/          Vue 3 + TypeScript + Tauri 2 (GUI)
  src-tauri/       Rust core: Bedrock, overlay, downloads, catalogs, accounts
nivora_launcher/   Python worker: lightweight operations and utilities
scripts/           Package builds (exe/deb/rpm/arch), diagnostics
docs/              Subsystem documentation
```

Everything critical to launching and performance lives in Rust; Python
handles administrative functions. Both parts carry a build stamp — if
they disagree, the launcher shows a warning instead of behaving
unpredictably.

## Data & privacy

- Data directories: `nivora-launcher` under `%APPDATA%`/`%LOCALAPPDATA%`
  (Windows) or the XDG directories (Linux). Data from the legacy
  `trial-launcher` directory is migrated automatically on first launch.
- Credentials live in the system store (Credential Manager / keyring);
  Microsoft passwords are entered on Microsoft pages and never reach
  the launcher.
- No telemetry. All requests go to official APIs (Mojang, Microsoft,
  mod catalogs) and to your proxy/cloud if you configure one.

## Hotkeys

| Action | Default |
|---|---|
| Game overlay | `Shift+Tab` (configurable) |
| Overlay video recording | `F7` (configurable) |
| Search / Settings / Refresh | `Ctrl+K` / `Ctrl+,` / `Ctrl+R` |
| New activity panel | `Ctrl+J` |

## Diagnostics

- “Overlay diagnostics” in settings — a step-by-step self test
  (environment, window, visibility, always-on-top, global hotkeys).
- Windows: `scripts/collect_crash_report.bat` collects the logs into a
  single file (`%LOCALAPPDATA%\nivora-launcher\`).
- Startup failures are shown as a dialog with the error text, never
  silently.

## Contributing

PRs are welcome: describe the problem and the scenario, keep the tests
green (`cargo test`, the Python suite, `npm run build`). New crate
dependencies are discussed separately. The UI language is Russian; the
README is bilingual.

## License

GPL-3.0. See [LICENSE](LICENSE).

**Minecraft** is a trademark of Mojang Studios / Microsoft. The launcher
is not affiliated with them, does not resell the game and requires a
purchased copy: a Microsoft account with a Java license, and for Bedrock
an account owning “Minecraft for Windows”. BedrockOnLinux is a separate
project, [Wyze3306/BedrockOnLinux](https://github.com/Wyze3306/BedrockOnLinux).
