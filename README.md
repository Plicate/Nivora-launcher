<div align="center">

# Nivora Launcher

**A universal Minecraft launcher: Java, Bedrock and servers — in one window**

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue)]()
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-green)]()
[![Tauri 2](https://img.shields.io/badge/Tauri-2-orange)]()
[![Version](https://img.shields.io/badge/version-1.0.0-purple)]()

**[English](README.md)** | **[Русский](README.ru.md)**

A modern Vue 3 interface, a fast Rust core, and an honest approach to
licenses and your data. Bedrock is Windows/Linux only; everything else
works on all three desktop platforms.

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
  tags, icons, playtime, duplication, ZIP export; instance shortcuts on
  the desktop and in the app menu (`.lnk` / `.app` / `.desktop`) that
  start the launcher and launch that exact instance, plus one-click
  Markdown/CSV/text export of an instance's mod list.
- **Catalogs** — mods, resource packs, shaders, data packs and modpacks
  from Modrinth, CurseForge, FTB and Technic; server plugins from
  Modrinth, Hangar, Spiget and GitHub Releases.
- **Accounts** — Microsoft device-code sign-in (Xbox Live/XSTS, skins and
  capes) and ely.by login; multiple accounts with aliases. Passwords are
  never stored: tokens live in the system credential store.
- **In-game overlay** (`Shift+Tab`) — mods, session time, system metrics
  and video recording (`F7`) on top of the game.
- **Workspace** — live console, snapshots, screenshot gallery, file
  manager, CPU/RAM/disk metrics.
- **Personalization** — a theme center in the spirit of Blender/Unity:
  colors, shapes, wallpaper, fonts, layout, animations; themes from a
  GitHub catalog.
- **Its own file windows** — folder browsing and file picking happen in
  launcher dialogs instead of the OS explorer and native pickers; the
  right-click menu is the launcher's own, with real actions for instances,
  accounts and modpacks.
- **Native Rust core** — installs, version/loader/server-core metadata,
  Bedrock, server management (start/stop/console), server networking
  (public-IP diagnostics, UPnP port publishing, TCP traffic counters),
  instance files & logs, themes, backups and update checks all run inside
  the Tauri process through the `rust_engine`; the Python worker only
  serves the remaining catalog and plugin operations. Progress of native
  downloads streams to the ACTIVITY panel live.
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
| **macOS 10.15+** | `.dmg` images for Apple Silicon and Intel. Not notarized: first launch after downloading — right-click → Open (or `xattr -dr com.apple.quarantine "Nivora Launcher.app"`). |

Ready-made packages are published on the Releases page; the launcher
checks for new versions on its own (on macOS updates are manual: the
updater feed requires a Developer ID signature we don't have).

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

The source archives are not incremental: unpack them into a **clean
directory** (`rm -rf nivora-launcher` first) — files removed in newer
versions stay behind otherwise and break the TypeScript build.

```bash
git clone <repo> nivora-launcher && cd nivora-launcher
# same rule for a source archive:
# rm -rf nivora-launcher && tar -xzf nivora-launcher-stage4.tar.gz

# Windows (from WSL/a Linux cross environment, or from Windows)
bash scripts/build_windows_exe.sh

# Linux
bash scripts/build_deb.sh            # или rpm/arch, см. scripts/build_all.sh

# macOS
bash scripts/build_macos_app.sh       # MACOS_ARCH=arm64|x86_64|both
# → dist/macos/Nivora-Launcher_<версия>_<arch>.dmg (+ .app.zip, SHA256SUMS)

# Всё сразу для текущей ОС
bash scripts/build_all.sh             # проверяет артефакт каждой цели

# Development
cd frontend && npm install && npm run tauri dev
```

The Python backend requires Python 3.10+. A bare `npm run tauri build`
without the backend step works for development (the launcher falls back
to the system `python3`), but release packages always bundle the
PyInstaller-built backend. Pre-release checks:

```bash
cargo test --locked                                # Rust core
python3 -m unittest discover -s tests -q           # Python side
cd frontend && npm run build                       # vue-tsc + vite
cd frontend && npm test                            # мост/реестр операций
```

A packaged app can smoke-test itself end-to-end (the same bridge the UI
uses, parallel native/worker calls, a report and an exit code):

```bash
./nivora-launcher.exe --smoke-test        # Windows; отчёт: %LOCALAPPDATA%\nivora-launcher\smoke-test.json
./Nivora\ Launcher.app/Contents/MacOS/nivora-launcher --smoke-test
nivora-launcher --smoke-test              # Linux (отчёт: ~/.local/share/nivora-launcher/)
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
  src-tauri/       Rust engine: bootstrap, settings, instance files/logs,
                   groups, plugins, Bedrock, overlay, downloads, accounts,
                   ely.by login, shader analysis, global search, world import,
                   instance optimization, Modrinth catalog
  cpp-turbo/       nivora-turbo — batch SHA-256 and jar-manifest scanning
                   (C++, only system zlib)
  go-modcat/       nivora-modcat — Modrinth catalog in Go: search, project
                   cards and concurrent update checks
  java/            ModScan.java — deep mod inspection: every manifest format
                   plus the required Java version from class files
nivora_launcher/   Python worker: modpack import/export, cloud sync,
                   plugin infrastructure — background work only
scripts/           Package builds (exe/deb/rpm/arch/app+dmg), diagnostics
docs/              Subsystem documentation
```

Startup runs entirely on the Rust engine — `bootstrap`, settings and the
Files/Logs tabs never spawn Python. The worker is started lazily, only
for operations the engine does not handle natively yet (catalog lookups,
modpack assembly, WebDAV cloud sync), so a long download no longer
queues behind UI requests. Heavy hashing inside the worker (cloud
manifests) is delegated to the `nivora-turbo` helper binary: a thread
pool with hardware SHA where available, verified against reference
vectors at build time and falling back to `hashlib` when absent.

Why this split of languages: everything in-process belongs to Rust (no
second runtime, no GC pauses, direct Tauri APIs). The other languages
live where a helper process pays for itself: C++ (`nivora-turbo`) — batch
file jobs (SHA-256, jar-manifest scanning) with no runtime attached;
Go (`nivora-modcat`) — the network-heavy Modrinth catalog with concurrent
requests; Java (`ModScan.java`) — the native habitat of jars: manifest
parsing plus class files for an honest "required Java version". A missing
helper never breaks the launcher — the engine falls back to its old path.

Instance installs download everything at once: client jar, libraries,
assets and the log config run as four parallel stages, each with its own
connection pool (large files with a known hash arrive in segments), while
a shared integrity journal keeps the stages from clobbering each other's
records. Global search (Ctrl+K) also looks inside jars, so a mod is found
even when its file name says nothing about its identity.

Both parts carry a build stamp — if they disagree, the launcher shows a
warning instead of behaving unpredictably.

## Data & privacy

- Data directories: `nivora-launcher` under `%APPDATA%`/`%LOCALAPPDATA%`
  (Windows) or the XDG directories (Linux **and macOS** — one layout for
  both keeps the Rust core, the Python worker and the asset protocol in
  sync; on macOS that means `~/.local/share`, `~/.config`, `~/.cache`).
  Data from the legacy `trial-launcher` directory is migrated
  automatically on first launch.
- Credentials live in the system store (Credential Manager / keyring);
  Microsoft passwords are entered on Microsoft pages and never reach
  the launcher.
- No telemetry. All requests go to official APIs (Mojang, Microsoft,
  ely.by, mod catalogs) and to your proxy/cloud if you configure one.
- The built-in file browser only reaches the home directory, the game
  folder and the launcher's own data — nothing else leaves those roots.

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
