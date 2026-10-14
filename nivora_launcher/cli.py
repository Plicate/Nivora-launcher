from __future__ import annotations

import argparse
import json
import platform
import signal
import shutil
import sys
from dataclasses import asdict
from pathlib import Path

from . import __version__
from .accounts import AccountStore, MicrosoftAuthenticator, OfflineDebugAccount
from .config import ConfigStore
from .errors import LauncherError
from .installer import MinecraftInstaller
from .instance_workspace import InstanceWorkspace
from .instances import InstanceStore
from .launcher import GameLauncher
from .loaders import LoaderRepository
from .metadata import VersionRepository, safe_instance_name
from .mods import ModrinthProvider
from .net import HttpClient
from .runtime import java_version
from .skins import SkinStore, apply_cape, apply_skin, clear_cape, profile


class _DownloadLogger:
    def __init__(self) -> None:
        self.last_percent: dict[str, int] = {}

    def __call__(self, event: str, url: str, destination: Path, current: int, total: int | None) -> None:
        key = str(destination)
        if event == "start":
            self.last_percent[key] = -10
            print(f"[download] {url} -> {destination}", flush=True)
            return
        if event == "progress" and total:
            percent = min(100, int(current * 100 / total))
            previous = self.last_percent.get(key, -10)
            if percent == 100 or percent - previous >= 10:
                self.last_percent[key] = percent
                print(f"[download] {destination.name}: {percent}% ({current}/{total} bytes)", flush=True)
            return
        if event == "cached":
            print(f"[cache] {url} -> {destination}", flush=True)
            return
        if event == "complete":
            print(f"[downloaded] {destination} ({current} bytes)", flush=True)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="nivora-launcher", description="Nivora Launcher — универсальный лаунчер Minecraft")
    parser.add_argument("--version", action="version", version=__version__)
    parser.add_argument("--gui", action="store_true", help="Открыть графический интерфейс")
    commands = parser.add_subparsers(dest="command", required=False)

    versions = commands.add_parser("versions", help="Показать доступные версии Minecraft")
    versions.add_argument("--releases-only", action="store_true")
    versions.add_argument("--limit", type=int, default=40)

    loaders = commands.add_parser("loaders", help="Показать версии загрузчика")
    loader_commands = loaders.add_subparsers(dest="loaders_command", required=True)
    loader_choices = loader_commands.add_parser("choices")
    loader_choices.add_argument("loader", choices=["fabric", "forge", "neoforge", "quilt"])
    loader_choices.add_argument("minecraft_version")

    for name in ("install", "launch"):
        command = commands.add_parser(name, help=f"{name.capitalize()} Minecraft")
        command.add_argument("version", help="например 1.20.1 или 26.3")
        command.add_argument("--loader", choices=["vanilla", "fabric", "forge", "neoforge", "quilt"], default="vanilla")
        command.add_argument("--loader-version", default=None)
        command.add_argument("--instance", default=None)
        command.add_argument("--no-assets", action="store_true", help="не скачивать отдельные asset objects")
        command.add_argument("--memory", default=None, help="временное значение Xmx, например 4096MiB или 4GiB")
        if name == "launch":
            command.add_argument("--account", choices=["auto", "microsoft", "offline"], default="auto")
            command.add_argument("--dry-run", action="store_true")

    login = commands.add_parser("login", help="Авторизация")
    login_commands = login.add_subparsers(dest="login_command", required=True)
    login_commands.add_parser("microsoft", help="Войти через Microsoft device code")

    account = commands.add_parser("account", help="Локальные аккаунты")
    account_commands = account.add_subparsers(dest="account_command", required=True)
    account_commands.add_parser("list")
    debug = account_commands.add_parser("debug", help="Создать бессрочный offline-аккаунт для отладки")
    debug.add_argument("username")
    offline = account_commands.add_parser("offline", help="Создать бессрочный автономный аккаунт")
    offline.add_argument("username")
    logout = account_commands.add_parser("logout")
    logout.add_argument("account_id", nargs="?", default="microsoft")
    default_account = account_commands.add_parser("default", aliases=["set-default"], help="Выбрать аккаунт по умолчанию")
    default_account.add_argument("account_id")
    edit_account = account_commands.add_parser("edit")
    edit_account.add_argument("account_id")
    edit_account.add_argument("alias")

    config = commands.add_parser("config", help="Настройки Nivora Launcher")
    config_commands = config.add_subparsers(dest="config_command", required=True)
    config_show = config_commands.add_parser("show")
    config_show.add_argument("--raw", action="store_true", help="Показать секретные значения локальному интерфейсу")
    config_set = config_commands.add_parser("set")
    config_set.add_argument("key")
    config_set.add_argument("value")

    mods = commands.add_parser("mods", help="Поиск и установка модов")
    mods_commands = mods.add_subparsers(dest="mods_command", required=True)
    search = mods_commands.add_parser("search")
    search.add_argument("source", choices=["modrinth"])
    search.add_argument("query")
    search.add_argument("--game-version", default=None)
    search.add_argument("--loader", default=None)
    search.add_argument("--limit", type=int, default=10)
    install_mod = mods_commands.add_parser("install")
    install_mod.add_argument("source", choices=["modrinth"])
    install_mod.add_argument("project")
    install_mod.add_argument("--instance", required=True)
    install_mod.add_argument("--game-version", required=True)
    install_mod.add_argument("--loader", default=None)

    plugin = commands.add_parser("plugin", help="Plugin author tools")
    plugin_commands = plugin.add_subparsers(dest="plugin_command", required=True)
    plugin_reload = plugin_commands.add_parser("reload", help="Request a safe developer runtime reload")
    plugin_reload.add_argument("plugin_id")
    plugin_init = plugin_commands.add_parser("init", help="Create a v3 plugin starter")
    plugin_init.add_argument("directory")
    plugin_lint = plugin_commands.add_parser("lint", help="Validate a plugin project")
    plugin_lint.add_argument("directory")
    plugin_pack = plugin_commands.add_parser("pack", help="Create a deterministic plugin ZIP")
    plugin_pack.add_argument("directory")
    plugin_pack.add_argument("output")
    plugin_pack.add_argument("--key", default=None, help="Ed25519 private PEM for signing")
    plugin_verify = plugin_commands.add_parser("verify", help="Verify a plugin ZIP")
    plugin_verify.add_argument("archive")
    plugin_keygen = plugin_commands.add_parser("keygen", help="Generate an Ed25519 signing key pair")
    plugin_keygen.add_argument("private_pem")
    plugin_keygen.add_argument("public_pem")

    skins = commands.add_parser("skins", help="Локальная библиотека и Microsoft-скины")
    skin_commands = skins.add_subparsers(dest="skins_command", required=True)
    skin_commands.add_parser("list")
    skin_import = skin_commands.add_parser("import")
    skin_import.add_argument("source")
    skin_import.add_argument("--name", default=None)
    skin_import.add_argument("--model", choices=["classic", "slim"], default="classic")
    skin_apply = skin_commands.add_parser("apply")
    skin_apply.add_argument("account_id")
    skin_apply.add_argument("skin_id")
    skin_apply.add_argument("--model", choices=["classic", "slim"], default="classic")
    skin_profile = skin_commands.add_parser("profile")
    skin_profile.add_argument("account_id")
    cape_apply = skin_commands.add_parser("cape-apply")
    cape_apply.add_argument("account_id")
    cape_apply.add_argument("cape_id")
    cape_clear = skin_commands.add_parser("cape-clear")
    cape_clear.add_argument("account_id")

    open_folder = commands.add_parser("open-folder", help="Открыть папку игры")
    open_folder.add_argument("path", nargs="?", default=None)
    commands.add_parser("doctor", help="Проверить окружение")

    audit = commands.add_parser("audit", help="Проверить безопасность мода или плагина")
    audit.add_argument("path", help="путь к .jar файлу мода или серверного плагина")
    audit.add_argument("--yes", action="store_true", help="не спрашивать подтверждение проверки")
    audit.add_argument("--json", action="store_true", help="вывести полный отчёт в JSON")
    audit.add_argument("--verbose", action="store_true", help="показывать информационные находки")

    instances = commands.add_parser("instances", help="Управление экземплярами")
    instance_commands = instances.add_subparsers(dest="instances_command", required=True)
    instance_commands.add_parser("list")
    delete_instance = instance_commands.add_parser("delete")
    delete_instance.add_argument("name")
    duplicate = instance_commands.add_parser("duplicate")
    duplicate.add_argument("name")
    duplicate.add_argument("target")
    export = instance_commands.add_parser("export")
    export.add_argument("name")
    export.add_argument("destination")
    shortcut = instance_commands.add_parser("shortcut")
    shortcut.add_argument("name")
    shortcut.add_argument("--directory", default=None)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)
    try:
        if args.gui or args.command is None:
            raise LauncherError(
                "Графический интерфейс запускается отдельным desktop-приложением. "
                "Запустите установленную команду trial-launcher или бинарник trial-launcher-gui."
            )
        return _dispatch(args)
    except LauncherError as exc:
        print(f"Ошибка: {exc}", file=sys.stderr)
        return 2
    except (OSError, ValueError, TypeError) as exc:
        print(f"Ошибка: {exc}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("\nОперация отменена.", file=sys.stderr)
        return 130


def _dispatch(args: argparse.Namespace) -> int:
    if args.command == "plugin":
        return _plugin_command(args)

    config = ConfigStore()
    http = HttpClient(download_observer=_DownloadLogger())
    accounts = AccountStore(config)
    repository = VersionRepository(config, http)
    installer = MinecraftInstaller(config, repository=repository, http=http)
    launcher = GameLauncher(config, installer, accounts, repository)
    instances = InstanceStore(config)

    if args.command == "versions":
        entries = repository.list_versions(include_snapshots=not args.releases_only)
        for entry in entries[: max(0, args.limit)]:
            print(f"{entry.get('id',''):<24} {entry.get('type',''):<9} {entry.get('releaseTime','')}")
        return 0
    if args.command == "loaders" and args.loaders_command == "choices":
        for choice in LoaderRepository(http).choices(args.loader, args.minecraft_version):
            print(choice.version)
        return 0
    if args.command == "install":
        if args.memory:
            config.set_value("memory", args.memory)
        result = installer.install(
            args.version,
            loader=args.loader,
            loader_version=args.loader_version,
            instance_name=args.instance,
            assets=not args.no_assets,
        )
        print(f"Установлено: {result.version_id}")
        print(f"Instance: {result.instance_dir}")
        return 0
    if args.command == "launch":
        if args.memory:
            config.set_value("memory", args.memory)
        result = installer.install(
            args.version,
            loader=args.loader,
            loader_version=args.loader_version,
            instance_name=args.instance,
            assets=not args.no_assets,
        )
        account = _select_account(args.account, config, accounts)
        launch_settings = None
        if args.instance:
            launch_settings = InstanceWorkspace(config, instances).settings(args.instance)
            if args.memory:
                launch_settings["memory"] = args.memory
        previous_term = signal.getsignal(signal.SIGTERM)
        signal.signal(signal.SIGTERM, lambda _signum, _frame: launcher.stop())
        try:
            return launcher.run(result, account, dry_run=args.dry_run, settings=launch_settings)
        finally:
            signal.signal(signal.SIGTERM, previous_term)
    if args.command == "login" and args.login_command == "microsoft":
        account = MicrosoftAuthenticator(config, accounts, http).login_device_code()
        print(f"Выполнен вход: {account.username} ({account.uuid})")
        return 0
    if args.command == "account":
        return _account_command(args, config, accounts)
    if args.command == "instances":
        return _instances_command(args, instances)
    if args.command == "config":
        return _config_command(args, config)
    if args.command == "mods":
        return _mods_command(args, config, http)
    if args.command == "skins":
        return _skins_command(args, config, accounts, http)
    if args.command == "open-folder":
        launcher.open_folder(Path(args.path).expanduser() if args.path else None)
        return 0
    if args.command == "audit":
        return _audit_command(args, config)
    if args.command == "doctor":
        return _doctor(config)
    raise LauncherError("Неизвестная команда")


def _audit_command(args: argparse.Namespace, config: ConfigStore) -> int:
    from .mod_audit import AuditError, ModAuditStore

    store = ModAuditStore(config.paths.data_home / "mod-audit")
    try:
        payload = store.prepare(args.path)
    except AuditError as exc:
        print(f"Ошибка: {exc}", file=sys.stderr)
        return 2
    estimate = payload["estimate"]
    structure = payload["structure"]
    source = payload["source"]
    print(f"Файл: {source['path']}")
    print(f"SHA-256: {source['sha256']}")
    print(f"Платформа: {structure['platform']}; классов: {structure['classes']}; строк-констант: {structure['strings']}")
    if structure["natives"]:
        print(f"Внимание: нативные библиотеки: {', '.join(structure['natives'][:4])}")
    print(f"Проверок будет выполнено: {payload['checks_count']}")
    print(
        f"Оценка времени проверки: {estimate['label']} "
        f"(структура: {estimate['source_lines']} строк исходников, "
        f"скорость этого ПК: {estimate['measured_lines_per_second']} строк/с)"
    )

    accepted = args.yes
    if not accepted:
        try:
            answer = input("Начать проверку? [y/N] ").strip().lower()
        except (KeyboardInterrupt, EOFError, OSError, RuntimeError):
            # EOFError/OSError — неинтерактивный запуск; RuntimeError —
            # windowed-exe (PyInstaller --noconsole), где sys.stdin отсутствует.
            print()
            accepted = False
        else:
            accepted = answer in {"y", "yes", "д", "да"}
    if not accepted:
        store.cancel(payload["session_id"])
        print("Проверка отклонена: декомпилированные исходники уничтожены без анализа.")
        return 0

    report = store.accept(payload["session_id"])
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 0

    print()
    print(f"Вердикт: {report['verdict_label']} (риск {report['risk_score']}/100)")
    counts = report["counts"]
    print(
        f"Находки: опасных {counts['critical']}, подозрительных {counts['warning']}, "
        f"замечаний {counts['notice']}, информационных {counts['info']}"
    )
    for finding in report["findings"]:
        if finding["severity"] == "info" and not args.verbose:
            continue
        label = {"critical": "ОПАСНО", "warning": "ПОДОЗРИТЕЛЬНО", "notice": "ЗАМЕЧАНИЕ", "info": "ИНФО"}[finding["severity"]]
        print()
        print(f"[{label}] {finding['title']}")
        print(f"  {finding['detail']}")
        if finding["evidence"]:
            shown = ", ".join(item["class"] for item in finding["evidence"][:4])
            print(f"  Где: {shown}")
        print(f"  Рекомендация: {finding['recommendation']}")
    print()
    print("Декомпилированные исходники уничтожены после проверки.")
    return 0


def _plugin_command(args: argparse.Namespace) -> int:
    if args.plugin_command == "reload":
        from .plugin_reload import request_reload
        request_reload(args.plugin_id)
        print("Reload requested; an open launcher will revalidate and restart the plugin.")
        return 0
    from . import plugin_dev

    if args.plugin_command == "init":
        directory = plugin_dev.init_plugin(args.directory)
        print(f"Plugin starter created: {directory}")
        return 0
    if args.plugin_command == "lint":
        warnings = plugin_dev.lint_plugin(args.directory)
        for warning in warnings:
            print(f"Warning: {warning}")
        print("Plugin lint passed.")
        return 0
    if args.plugin_command == "pack":
        archive = plugin_dev.pack_plugin(args.directory, args.output, key=args.key)
        print(f"Plugin archive created: {archive}")
        return 0
    if args.plugin_command == "verify":
        result = plugin_dev.verify_plugin(args.archive)
        signature_status = "signed" if result["signed"] else "unsigned; integrity verified"
        print(f"Verified {result['id']}@{result['version']} ({signature_status}).")
        return 0
    if args.plugin_command == "keygen":
        private_path, public_path = plugin_dev.generate_keypair(args.private_pem, args.public_pem)
        print(f"Ed25519 keys written: {private_path}, {public_path}")
        return 0
    raise LauncherError("Unknown plugin command")


def _select_account(kind: str, config: ConfigStore, accounts: AccountStore):
    if kind == "offline":
        return OfflineDebugAccount(config, accounts).get()
    if kind == "microsoft":
        return MicrosoftAuthenticator(config, accounts).get_valid()

    if kind != "auto":
        account = accounts.get(account_id=kind)
        if not account:
            raise LauncherError(f"Аккаунт не найден: {kind}")
        return MicrosoftAuthenticator(config, accounts).get_valid(kind) if account.kind == "microsoft" else account

    configured_id = str(getattr(config.settings, "default_account_id", "auto") or "auto")
    selected = accounts.get(account_id=configured_id) if configured_id.lower() != "auto" else None
    if selected:
        if selected.kind == "microsoft":
            return MicrosoftAuthenticator(config, accounts).get_valid(selected.account_id)
        if selected.kind == "offline":
            return OfflineDebugAccount(config, accounts).get()
        return selected

    default = accounts.default()
    if default:
        if default.kind == "microsoft":
            return MicrosoftAuthenticator(config, accounts).get_valid(default.account_id)
        if default.kind == "offline":
            return OfflineDebugAccount(config, accounts).get()
        return default

    microsoft = accounts.get(kind="microsoft")
    if microsoft:
        return MicrosoftAuthenticator(config, accounts).get_valid(microsoft.account_id)
    return OfflineDebugAccount(config, accounts).get()


def _account_command(args: argparse.Namespace, config: ConfigStore, accounts: AccountStore) -> int:
    if args.account_command == "list":
        for account in accounts.all():
            remaining = ""
            if account.kind == "offline":
                account = OfflineDebugAccount(config, accounts).get()
                remaining = ", без ограничения"
            display_name = account.alias or account.username
            print(f"{account.account_id}: {display_name} [{account.kind}]{remaining}")
        return 0
    if args.account_command in {"debug", "offline"}:
        account = OfflineDebugAccount(config, accounts).create(args.username)
        print(
            f"Создан debug-аккаунт {account.username}; доступ без ограничения"
            if account.limit_seconds <= 0
            else f"Создан debug-аккаунт {account.username}; доступно {account.limit_seconds // 60} минут"
        )
        return 0
    if args.account_command == "logout":
        print("Аккаунт удалён." if accounts.remove(args.account_id) else "Такой аккаунт не найден.")
        return 0
    if args.account_command in {"default", "set-default"}:
        account = accounts.set_default(args.account_id)
        print(f"Аккаунт по умолчанию: {account.alias or account.username}")
        return 0
    if args.account_command == "edit":
        account = accounts.set_alias(args.account_id, args.alias)
        print(f"Сохранено имя аккаунта: {account.alias or account.username}")
        return 0
    raise LauncherError("Неизвестная команда account")


def _instances_command(args: argparse.Namespace, instances: InstanceStore) -> int:
    if args.instances_command == "list":
        for value in instances.list():
            print(f"{value.name}\t{value.version_id or '-'}\t{value.loader}\t{value.path}")
        return 0
    if args.instances_command == "delete":
        instances.delete(args.name)
        print(f"Удалён экземпляр: {args.name}")
        return 0
    if args.instances_command == "duplicate":
        value = instances.duplicate(args.name, args.target)
        print(f"Создан экземпляр: {value.name}")
        return 0
    if args.instances_command == "export":
        path = instances.export_zip(args.name, Path(args.destination))
        print(f"Экспортирован: {path}")
        return 0
    if args.instances_command == "shortcut":
        path = instances.create_shortcut(args.name, Path(args.directory) if args.directory else None)
        print(f"Создан ярлык: {path}")
        return 0
    raise LauncherError("Неизвестная команда instances")


def _config_command(args: argparse.Namespace, config: ConfigStore) -> int:
    if args.config_command == "show":
        value = asdict(config.settings)
        if not args.raw:
            for secret in ("client_id",):
                if value[secret]:
                    value[secret] = value[secret][:4] + "..."
        print(json.dumps(value, ensure_ascii=False, indent=2))
        print(f"config: {config.paths.settings_file}")
        print(f"game: {config.game_dir}")
        return 0
    if args.config_command == "set":
        config.set_value(args.key, args.value)
        print(f"Сохранено: {args.key}")
        return 0
    raise LauncherError("Неизвестная команда config")


def _mods_command(args: argparse.Namespace, config: ConfigStore, http: HttpClient) -> int:
    provider = ModrinthProvider(http)
    if args.mods_command == "search":
        values = provider.search(args.query, args.game_version, args.loader, args.limit)
        for item in values:
            print(f"{item.get('project_id', item.get('id','')):<12} {item.get('title', item.get('name',''))}")
        return 0
    if args.mods_command == "install":
        instance = config.game_dir / "instances" / safe_instance_name(args.instance)
        result = provider.install(args.project, instance, args.game_version, args.loader)
        print(f"Установлен мод: {result.name} -> {result.path}")
        return 0
    raise LauncherError("Неизвестная команда mods")


def _skins_command(args: argparse.Namespace, config: ConfigStore, accounts: AccountStore, http: HttpClient) -> int:
    store = SkinStore(config)
    if args.skins_command == "list":
        for skin in store.list():
            print(f"{skin.skin_id}\t{skin.name}\t{skin.model}\t{skin.path}")
        return 0
    if args.skins_command == "import":
        skin = store.import_file(Path(args.source), args.name, args.model)
        print(f"{skin.skin_id}\t{skin.name}\t{skin.model}\t{skin.path}")
        return 0
    if args.skins_command == "apply":
        account = _valid_microsoft_account(args.account_id, config, accounts, http)
        apply_skin(http, account, store.get(args.skin_id), args.model)
        print(f"Скин установлен: {args.skin_id} ({args.model})")
        return 0
    if args.skins_command == "profile":
        account = _valid_microsoft_account(args.account_id, config, accounts, http)
        value = profile(http, account)
        for cape in value.get("capes", []):
            if isinstance(cape, dict) and cape.get("id"):
                print(f"{cape['id']}\t{cape.get('name', cape['id'])}\t{cape.get('alias', '')}\t{str(cape.get('state', '')).lower()}")
        return 0
    if args.skins_command == "cape-apply":
        apply_cape(http, _valid_microsoft_account(args.account_id, config, accounts, http), args.cape_id)
        print(f"Плащ установлен: {args.cape_id}")
        return 0
    if args.skins_command == "cape-clear":
        clear_cape(http, _valid_microsoft_account(args.account_id, config, accounts, http))
        print("Плащ снят")
        return 0
    raise LauncherError("Неизвестная команда skins")


def _valid_microsoft_account(account_id: str, config: ConfigStore, accounts: AccountStore, http: HttpClient):
    account = accounts.get(account_id=account_id)
    if not account or account.kind != "microsoft":
        raise LauncherError("Нужен Microsoft-аккаунт")
    return MicrosoftAuthenticator(config, accounts, http).get_valid(account.account_id)


def _doctor(config: ConfigStore) -> int:
    java = shutil.which("java")
    xdg = shutil.which("xdg-open")
    print(f"platform: {platform.system()} {platform.machine()}")
    print(f"python: {sys.version.split()[0]}")
    print(f"java: {java or 'не найден'}")
    if java:
        print(f"java major: {java_version(Path(java)) or 'не определён'}")
    print(f"xdg-open: {xdg or 'не найден'}")
    print(f"config: {config.paths.config_home}")
    print(f"game: {config.game_dir}")
    return 0
