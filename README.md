# Nivora Launcher

A desktop launcher for Minecraft Java Edition, servers and Bedrock on supported platforms. The interface uses Vue 3 and Tauri 2; the application engine is written in Rust.

**English** | [Русский](README.ru.md)

## What it does

- Manage separate game instances, accounts, versions and loaders.
- Install mods, resource packs, shaders, data packs and modpacks from configured catalogs.
- Create and manage Minecraft servers.
- Use an in-game overlay, screenshots, activity logs and instance workspaces.
- Customize the interface and sync selected data through WebDAV.
- Run Bedrock through official Microsoft Store sources on Windows or BedrockOnLinux on Linux. Linux Bedrock may require Python 3 or the `bedrock-on-linux` executable.

Bedrock requires a valid Microsoft license. Nivora does not distribute game files.

## Install

Use a package provided for your platform: Windows installer, Linux package, or macOS disk image. Bedrock is supported only on Windows and Linux.

## Build and test

Install the platform prerequisites for Tauri, then run:

```bash
cd frontend
npm ci
npm run tauri dev
```

Frontend checks:

```bash
cd frontend
npm test
npm run build
```

Rust tests:

```bash
cd frontend/src-tauri
cargo test --locked
```

Platform packaging scripts are in `scripts/`; see [build instructions](docs/BUILDING.md) for prerequisites and targets.

## Project documentation

- [Security model](docs/security.md)
- [Plugin API](docs/plugin-api.md)
- [Theme format](docs/theme-format.md)
- [Server runtime](docs/server-runtime.md) and [networking](docs/server-network.md)
- [Cloud sync](docs/cloud-sync.md)
- [Release process](docs/RELEASING.md)

## License

GPL-3.0. See [LICENSE](LICENSE).

Minecraft is a trademark of Mojang Studios and Microsoft. Nivora Launcher is not affiliated with them. BedrockOnLinux is a separate project: [Wyze3306/BedrockOnLinux](https://github.com/Wyze3306/BedrockOnLinux).
