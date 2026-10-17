from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import tempfile
import time
import uuid
import webbrowser
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING
from typing import Any, Callable

from .config import ConfigStore
from .errors import AuthenticationError, LauncherError
from .net import HttpClient
from .platform_support import open_path, set_private_fd, set_private_path

if TYPE_CHECKING:
    from .operations import CancellationToken

MSA_DEVICE_CODE_URL = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode"
MSA_TOKEN_URL = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token"
XBOX_USER_URL = "https://user.auth.xboxlive.com/user/authenticate"
XSTS_URL = "https://xsts.auth.xboxlive.com/xsts/authorize"
LAUNCHER_LOGIN_URL = "https://api.minecraftservices.com/launcher/login"
ENTITLEMENTS_URL = "https://api.minecraftservices.com/entitlements/mcstore"
LICENSE_URL = "https://api.minecraftservices.com/entitlements/license"
PROFILE_URL = "https://api.minecraftservices.com/minecraft/profile"
MSA_STATUS_FILE = "microsoft-login.json"


@dataclass
class Account:
    account_id: str
    kind: str
    username: str
    uuid: str
    access_token: str
    expires_at: float = 0
    refresh_token: str = ""
    xuid: str = ""
    used_seconds: int = 0
    limit_seconds: int = 0
    alias: str = ""
    is_default: bool = False

    @classmethod
    def from_dict(cls, value: dict[str, Any]) -> "Account":
        return cls(
            account_id=str(value.get("account_id", "")),
            kind=str(value.get("kind", "")),
            username=str(value.get("username", "")),
            uuid=str(value.get("uuid", "")),
            access_token=str(value.get("access_token", "")),
            expires_at=float(value.get("expires_at", 0)),
            refresh_token=str(value.get("refresh_token", "")),
            xuid=str(value.get("xuid", "")),
            used_seconds=int(value.get("used_seconds", 0)),
            limit_seconds=int(value.get("limit_seconds", 0)),
            alias=str(value.get("alias", "")),
            is_default=bool(value.get("is_default", False)),
        )

    def to_dict(self) -> dict[str, Any]:
        value = {
            "account_id": self.account_id,
            "kind": self.kind,
            "username": self.username,
            "uuid": self.uuid,
            "expires_at": self.expires_at,
            "xuid": self.xuid,
            "used_seconds": self.used_seconds,
            "limit_seconds": self.limit_seconds,
            "alias": self.alias,
            "is_default": self.is_default,
        }
        if self.kind != "microsoft":
            value["access_token"] = self.access_token
            value["refresh_token"] = self.refresh_token
        return value


class AccountStore:
    def __init__(self, config: ConfigStore) -> None:
        self.config = config
        self.path = config.paths.accounts_file

    def _read(self) -> list[dict[str, Any]]:
        if not self.path.exists():
            return []
        try:
            if self.path.is_symlink() or self.path.stat().st_size > 4 * 1024 * 1024:
                raise LauncherError("Файл аккаунтов слишком большой или является ссылкой")
            value = json.loads(self.path.read_text(encoding="utf-8"))
            return [item for item in value if isinstance(item, dict)] if isinstance(value, list) else []
        except (OSError, ValueError) as exc:
            raise LauncherError(f"Не удалось прочитать аккаунты: {exc}") from exc

    def all(self) -> list[Account]:
        records = self._read()
        accounts: list[Account] = []
        changed = False
        for item in records:
            account = Account.from_dict(item)
            if account.kind == "microsoft":
                legacy_tokens = bool(item.get("access_token") or item.get("refresh_token"))
                if legacy_tokens:
                    _store_account_tokens(account, self.path.parent)
                    records[records.index(item)] = account.to_dict()
                    changed = True
                else:
                    tokens = _load_account_tokens(account.account_id, self.path.parent)
                    account.access_token = tokens.get("access_token", "")
                    account.refresh_token = tokens.get("refresh_token", "")
            accounts.append(account)
        if changed:
            _write_private_json(self.path, records)
        return accounts

    def get(self, account_id: str | None = None, kind: str | None = None) -> Account | None:
        accounts = self.all()
        for account in accounts:
            if account_id and account.account_id == account_id:
                return account
        if kind:
            return next((account for account in accounts if account.kind == kind and account.is_default), None) or next(
                (account for account in accounts if account.kind == kind), None
            )
        return None

    def default(self) -> Account | None:
        return next((account for account in self.all() if account.is_default), None)

    def save(self, account: Account) -> None:
        records = self._read()
        if account.kind == "microsoft":
            _store_account_tokens(account, self.path.parent)
        replacement = account.to_dict()
        for index, item in enumerate(records):
            if item.get("account_id") == account.account_id:
                records[index] = replacement
                _write_private_json(self.path, records)
                return
        if not records:
            account.is_default = True
            replacement = account.to_dict()
        records.append(replacement)
        _write_private_json(self.path, records)

    def remove(self, account_id: str) -> bool:
        records = self._read()
        updated = [item for item in records if item.get("account_id") != account_id]
        if len(updated) == len(records):
            return False
        _write_private_json(self.path, updated)
        if any(item.get("account_id") == account_id and item.get("kind") == "microsoft" for item in records):
            _remove_file_tokens(self.path.parent, account_id)
            try:
                _clear_account_tokens(account_id)
            except AuthenticationError:
                # Без secret-tool токены могли жить только в резервном файле.
                if os.name == "nt" or shutil.which("secret-tool"):
                    raise
        return True

    def set_alias(self, account_id: str, alias: str) -> Account:
        account = self.get(account_id=account_id)
        if not account:
            raise LauncherError(f"Аккаунт не найден: {account_id}")
        account.alias = alias.strip()[:40]
        self.save(account)
        return account

    def set_default(self, account_id: str) -> Account:
        records = self._read()
        selected: dict[str, Any] | None = None
        for item in records:
            is_selected = item.get("account_id") == account_id
            item["is_default"] = is_selected
            if is_selected:
                selected = item
        if selected is None:
            raise LauncherError(f"Аккаунт не найден: {account_id}")
        _write_private_json(self.path, records)
        self.config.set_value("default_account_id", account_id)
        return Account.from_dict(selected)

    def move(self, account_id: str, direction: str) -> list[Account]:
        records = self._read()
        index = next((index for index, item in enumerate(records) if item.get("account_id") == account_id), -1)
        if index < 0:
            raise LauncherError(f"Аккаунт не найден: {account_id}")
        target = index - 1 if direction == "up" else index + 1 if direction == "down" else index
        if 0 <= target < len(records):
            records[index], records[target] = records[target], records[index]
            _write_private_json(self.path, records)
        return [Account.from_dict(item) for item in records]


def validate_nickname(value: str) -> str:
    """Ник Minecraft: 3–16 символов, только латиница, цифры и «_».

    Недопустимые символы — ошибка, а не молчаливая обрезка: раньше через
    isalnum() проходила и кириллица.
    """
    value = str(value)
    if not value:
        raise LauncherError("Введите ник")
    invalid: list[str] = []
    for character in value:
        if not (character.isascii() and (character.isalnum() or character == "_")) and character not in invalid:
            invalid.append(character)
    if invalid:
        shown = [
            "«пробел»" if character == " " else "«пробельный символ»" if character.isspace() else f"«{character}»"
            for character in invalid[:6]
        ]
        if len(invalid) > 6:
            shown.append("…")
        raise LauncherError(f"Ник содержит недопустимые символы: {', '.join(shown)}. Разрешены только латинские буквы, цифры и _")
    if len(value) < 3:
        raise LauncherError("Ник должен быть не короче 3 символов")
    if len(value) > 16:
        raise LauncherError("Ник должен быть не длиннее 16 символов")
    return value


class OfflineDebugAccount:
    def __init__(self, config: ConfigStore, store: AccountStore) -> None:
        self.config = config
        self.store = store

    def create(self, username: str) -> Account:
        safe_name = validate_nickname(username)
        account = Account(
            account_id="offline-debug",
            kind="offline",
            username=safe_name,
            # Seed и маркер оффлайн-аккаунтов менять нельзя: UUID фигурирует
            # в playerdata серверов, смена оторвёт старые миры от аккаунтов.
            uuid=uuid.uuid5(uuid.NAMESPACE_URL, f"trial-launcher:{safe_name}").hex,
            access_token="trial-launcher-offline",
            limit_seconds=0,
        )
        self.store.save(account)
        return account

    def get(self) -> Account:
        account = self.store.get(account_id="offline-debug")
        if not account:
            raise LauncherError("Debug-аккаунт ещё не создан. Используйте: account debug <имя>")
        if account.limit_seconds > 0:
            account.limit_seconds = 0
            account.used_seconds = 0
            self.store.save(account)
        return account

    def remaining(self, account: Account | None = None) -> int:
        value = account or self.get()
        if value.limit_seconds <= 0:
            return 2**63 - 1
        return max(0, value.limit_seconds - value.used_seconds)

    def finish(self, account: Account, started_at: float) -> int:
        if account.limit_seconds <= 0:
            self.store.save(account)
            return self.remaining(account)
        elapsed = max(0, int(time.monotonic() - started_at))
        account.used_seconds = min(account.limit_seconds, account.used_seconds + elapsed)
        self.store.save(account)
        return self.remaining(account)


class MicrosoftAuthenticator:
    def __init__(
        self,
        config: ConfigStore,
        store: AccountStore,
        http: HttpClient | None = None,
        notify: Callable[[str], None] | None = None,
        cancellation: "CancellationToken | None" = None,
    ) -> None:
        self.config = config
        self.store = store
        self.http = http or HttpClient()
        self.notify = notify or print
        self.cancellation = cancellation

    def _check_cancelled(self) -> None:
        # Device-code вход ждёт пользователя до ~15 минут: без проверки
        # маркера остановки кнопка «Остановить» в тосте активности
        # оставалась в «Останавливаю…» до таймаута Microsoft.
        if self.cancellation is not None:
            self.cancellation.check()

    def _msa_endpoint(self, suffix: str) -> str:
        return f"{self.config.settings.login_api_url.rstrip('/')}/{suffix}"

    def login_device_code(self) -> Account:
        client_id = self.config.settings.client_id.strip()
        if not client_id:
            raise AuthenticationError("Microsoft-вход недоступен в этой сборке: public client ID не настроен")
        _write_microsoft_status(self.config, {"status": "requesting"})
        response = self.http.post_form(
            self._msa_endpoint("devicecode"),
            {"client_id": client_id, "scope": "XboxLive.signin offline_access"},
        )
        verification_uri = response.get("verification_uri") or response.get("verification_uri_complete")
        user_code = response.get("user_code")
        device_code = response.get("device_code")
        if not verification_uri or not user_code or not device_code:
            raise AuthenticationError("Microsoft не выдал корректный device code")
        _write_microsoft_status(
            self.config,
            {
                "status": "device_code",
                "code": str(user_code),
                "verification_uri": str(verification_uri),
                "expires_in": int(response.get("expires_in", 900)),
            },
        )
        self.notify(f"КОД MICROSOFT: {user_code}")
        self.notify(f"Откройте {verification_uri} и введите этот код в браузере.")
        # webbrowser внутри упакованного приложения под Wayland/XWayland порой молча
        # не находит браузер; явный xdg-open (как у ссылок плагинов) надёжнее.
        try:
            open_path(Path(str(verification_uri)))
        except (OSError, RuntimeError):
            try:
                webbrowser.open(str(verification_uri))
            except webbrowser.Error:
                self.notify("Не удалось открыть браузер автоматически — перейдите по ссылке вручную.")

        expires_in = int(response.get("expires_in", 900))
        interval = max(5, int(response.get("interval", 5)))
        deadline = time.monotonic() + expires_in
        msa = self._poll_msa_token(client_id, device_code, interval, deadline)
        self._check_cancelled()
        account = self._finish_microsoft_login(msa, client_id)
        profile_identity = account.uuid.replace("-", "").lower()
        existing = self.store.get(account_id=account.account_id)
        if not existing and profile_identity:
            existing = next(
                (
                    value
                    for value in self.store.all()
                    if value.kind == "microsoft" and value.uuid.replace("-", "").lower() == profile_identity
                ),
                None,
            )
        if existing:
            account.account_id = existing.account_id
            account.alias = existing.alias
            account.is_default = existing.is_default
        self.store.save(account)
        _write_microsoft_status(self.config, {"status": "completed"})
        return account

    def get_valid(self, account_id: str | None = None, *, force_refresh: bool = False) -> Account:
        account = self.store.get(account_id=account_id) if account_id else None
        if not account and not account_id:
            configured_id = str(getattr(self.config.settings, "default_account_id", "auto") or "auto")
            if configured_id.lower() != "auto":
                account = self.store.get(account_id=configured_id)
            if not account or account.kind != "microsoft":
                account = self.store.get(kind="microsoft")
        if not account or account.kind != "microsoft":
            raise AuthenticationError("Microsoft-аккаунт не найден. Используйте: login microsoft")
        if not force_refresh and account.expires_at > time.time() + 60 and account.access_token:
            return account
        if not account.refresh_token:
            if not account.access_token:
                store = "диспетчере учётных данных Windows" if os.name == "nt" else "Secret Service (gnome-keyring/KWallet)"
                raise AuthenticationError(
                    f"Токены Microsoft-аккаунта не найдены в {store}. Войдите заново; если ошибка повторяется — хранилище паролей недоступно."
                )
            raise AuthenticationError("Microsoft-токен истёк. Повторите login microsoft")
        msa = self._refresh_msa(account.refresh_token)
        refreshed = self._finish_microsoft_login(msa, self.config.settings.client_id)
        refreshed.account_id = account.account_id
        refreshed.alias = account.alias
        refreshed.is_default = account.is_default
        if not refreshed.refresh_token:
            refreshed.refresh_token = account.refresh_token
        self.store.save(refreshed)
        return refreshed

    def _poll_msa_token(self, client_id: str, device_code: str, interval: int, deadline: float) -> dict[str, Any]:
        while time.monotonic() < deadline:
            self._check_cancelled()
            status, response = self.http.post_form_soft(
                self._msa_endpoint("token"),
                {
                    "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                    "client_id": client_id,
                    "device_code": device_code,
                },
            )
            self._check_cancelled()
            if status == 200 and response.get("access_token"):
                return response
            error = response.get("error")
            if error == "authorization_pending":
                self.http.wait(interval)
                continue
            if error == "slow_down":
                interval += 5
                self.http.wait(interval)
                continue
            raise AuthenticationError(f"Microsoft OAuth завершился ошибкой: {error or 'неизвестная ошибка'}")
        raise AuthenticationError("Время ожидания входа Microsoft истекло")

    def _refresh_msa(self, refresh_token: str) -> dict[str, Any]:
        response = self.http.post_form(
            self._msa_endpoint("token"),
            {
                "client_id": self.config.settings.client_id,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "scope": "XboxLive.signin offline_access",
            },
        )
        if not response.get("access_token"):
            raise AuthenticationError("Не удалось обновить Microsoft-токен")
        return response

    def _finish_microsoft_login(self, msa: dict[str, Any], client_id: str) -> Account:
        xbl = self.http.post_json(
            XBOX_USER_URL,
            {
                "Properties": {
                    "AuthMethod": "RPS",
                    "SiteName": "user.auth.xboxlive.com",
                    "RpsTicket": f"d={msa['access_token']}",
                },
                "RelyingParty": "http://auth.xboxlive.com",
                "TokenType": "JWT",
            },
        )
        xsts = self.http.post_json(
            XSTS_URL,
            {
                "Properties": {"SandboxId": "RETAIL", "UserTokens": [xbl["Token"]]},
                "RelyingParty": "rp://api.minecraftservices.com/",
                "TokenType": "JWT",
            },
            headers={"x-xbl-contract-version": "1"},
        )
        if xsts.get("XErr"):
            raise AuthenticationError(f"Xbox XSTS отклонил аккаунт, код {xsts['XErr']}")
        uhs = xsts.get("DisplayClaims", {}).get("xui", [{}])[0].get("uhs")
        if not uhs or not xsts.get("Token"):
            raise AuthenticationError("Xbox XSTS не вернул пользовательский токен")
        minecraft = self.http.post_json(
            LAUNCHER_LOGIN_URL,
            {"platform": "PC_LAUNCHER", "xtoken": f"XBL3.0 x={uhs};{xsts['Token']}"},
        )
        token = minecraft.get("access_token")
        if not token:
            raise AuthenticationError("Minecraft Services не выдал access token")
        profile = self._get_profile(token)
        self._check_ownership(token)
        profile_id = str(profile.get("id") or "").strip()
        account_id = profile_id.lower()
        try:
            _account_secret_attributes(account_id)
        except AuthenticationError as exc:
            raise AuthenticationError("Minecraft Services не вернул корректный id профиля") from exc
        return Account(
            account_id=account_id,
            kind="microsoft",
            username=str(profile.get("name", minecraft.get("username", "Microsoft"))),
            uuid=profile_id,
            access_token=str(token),
            expires_at=time.time() + int(minecraft.get("expires_in", 86400)) - 120,
            refresh_token=str(msa.get("refresh_token", "")),
            xuid=str(minecraft.get("xuid", "")),
        )

    def _check_ownership(self, token: str) -> None:
        request_id = uuid.uuid4()
        # The current launcher endpoint needs the bearer header, so use the raw request here.
        raw = self.http.request(
            f"{LICENSE_URL}?requestId={request_id}",
            headers={"Authorization": f"Bearer {token}", "Accept": "application/json"},
        )
        try:
            response = json.loads(raw.decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise AuthenticationError("Minecraft Services вернул некорректные entitlement данные") from exc
        names = {item.get("name") for item in response.get("items", []) if isinstance(item, dict)}
        if not names.intersection({"game_minecraft", "product_minecraft"}):
            raise AuthenticationError("У Microsoft-аккаунта не найдено право на Minecraft Java Edition")

    def _get_profile(self, token: str) -> dict[str, Any]:
        raw = self.http.request(
            PROFILE_URL,
            headers={"Authorization": f"Bearer {token}", "Accept": "application/json"},
        )
        try:
            return json.loads(raw.decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise AuthenticationError("Minecraft Services вернул некорректный profile") from exc


def _write_private_json(path: Path, value: Any) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        set_private_fd(fd, stat.S_IRUSR | stat.S_IWUSR)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        temporary.replace(path)
    finally:
        if temporary.exists():
            temporary.unlink()


def _secret_tool() -> str:
    command = shutil.which("secret-tool")
    if not command:
        raise AuthenticationError("Для Microsoft-токенов нужны libsecret, secret-tool и работающий gnome-keyring")
    return command


def _account_secret_attributes(account_id: str) -> list[str]:
    if not account_id or not all(char.isalnum() or char in "._-" for char in account_id):
        raise AuthenticationError("Некорректный идентификатор Microsoft-аккаунта")
    # Имя сервиса secret-tool не переименовывать: у действующих пользователей
    # в keyring уже лежат токены под этим именем, смена = принудительный релогин.
    return ["service", "trial-launcher-microsoft", "account-id", account_id]


# Резервный файл токенов — формат общий с rust_engine/accounts.rs. Нужен, когда
# системное хранилище недоступно или временное: Secret Service без
# разблокированной связки «login» кладёт секреты в session-коллекцию, и после
# перезапуска токен Microsoft пропадал. Файл приватный (0600 / ACL пользователя).
TOKEN_FILE = "microsoft-tokens.json"


def _token_file(directory: Path) -> Path:
    return directory / TOKEN_FILE


def _read_file_tokens(directory: Path | None) -> dict[str, Any]:
    if directory is None:
        return {}
    path = _token_file(directory)
    try:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 4 * 1024 * 1024:
            return {}
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def _write_file_tokens(directory: Path, tokens: dict[str, Any]) -> None:
    path = _token_file(directory)
    if not tokens:
        if path.exists() and not path.is_symlink():
            path.unlink()
        return
    _write_private_json(path, tokens)
    set_private_path(path)


def _store_file_tokens(directory: Path, account_id: str, payload: dict[str, str]) -> None:
    _account_secret_attributes(account_id)
    tokens = _read_file_tokens(directory)
    tokens[account_id] = payload
    _write_file_tokens(directory, tokens)


def _remove_file_tokens(directory: Path | None, account_id: str) -> None:
    if directory is None:
        return
    tokens = _read_file_tokens(directory)
    if tokens.pop(account_id, None) is not None:
        _write_file_tokens(directory, tokens)


def _secret_is_persistent(account_id: str, payload: str) -> bool:
    if os.name == "nt":
        return _windows_load_token_payload(account_id) == payload
    try:
        search = _run_secret_tool(["search", "--all", *_account_secret_attributes(account_id)])
    except Exception:  # noqa: BLE001 — нет search: судим только по lookup
        search = None
    if search is not None and search.returncode == 0 and "/collection/session" in (search.stdout or ""):
        return False
    try:
        lookup = _run_secret_tool(["lookup", *_account_secret_attributes(account_id)])
    except (AuthenticationError, OSError, subprocess.SubprocessError):
        return False
    return lookup.returncode == 0 and lookup.stdout.rstrip("\n") == payload


def _load_account_tokens(account_id: str, directory: Path | None = None) -> dict[str, str]:
    tokens = _load_keyring_tokens(account_id)
    if tokens.get("access_token") or tokens.get("refresh_token"):
        return tokens
    stored = _read_file_tokens(directory).get(account_id)
    if isinstance(stored, dict):
        return {"access_token": str(stored.get("access_token", "")), "refresh_token": str(stored.get("refresh_token", ""))}
    return tokens


def _load_keyring_tokens(account_id: str) -> dict[str, str]:
    if os.name == "nt":
        try:
            raw = _windows_load_token_payload(account_id)
            value = json.loads(raw) if raw else {}
        except (AuthenticationError, ValueError, TypeError):
            return {}
        except Exception:
            return {}
        if not isinstance(value, dict):
            return {}
        return {
            "access_token": str(value.get("access_token", "")),
            "refresh_token": str(value.get("refresh_token", "")),
        }
    try:
        result = _run_secret_tool(["lookup", *_account_secret_attributes(account_id)])
    except (AuthenticationError, OSError, subprocess.SubprocessError):
        return {}
    if result.returncode != 0:
        return {}
    try:
        value = json.loads(result.stdout)
    except (ValueError, TypeError):
        return {}
    if not isinstance(value, dict):
        return {}
    return {
        "access_token": str(value.get("access_token", "")),
        "refresh_token": str(value.get("refresh_token", "")),
    }


def _store_account_tokens(account: Account, directory: Path | None = None) -> None:
    if account.kind != "microsoft" or not account.access_token:
        raise AuthenticationError("Microsoft access token отсутствует; повторите вход")
    payload = json.dumps(
        {"access_token": account.access_token, "refresh_token": account.refresh_token},
        ensure_ascii=False,
        separators=(",", ":"),
    )
    if directory is None:
        _store_keyring_tokens(account, payload)
        return
    try:
        _store_keyring_tokens(account, payload)
        persistent = _secret_is_persistent(account.account_id, payload)
    except AuthenticationError:
        persistent = False
    if persistent:
        _remove_file_tokens(directory, account.account_id)
    else:
        _store_file_tokens(directory, account.account_id, {"access_token": account.access_token, "refresh_token": account.refresh_token})


def _store_keyring_tokens(account: Account, payload: str) -> None:
    if os.name == "nt":
        _account_secret_attributes(account.account_id)
        if not _windows_write_chunked(_windows_credential_target(account.account_id), account.account_id, payload):
            raise AuthenticationError("Не удалось сохранить Microsoft-токен в диспетчере учётных данных Windows")
        return
    try:
        # Secret Service allows duplicate items with the same attributes on
        # some keyring implementations. Clear first so lookup cannot return a
        # stale Microsoft token after a refresh or re-login.
        _clear_account_tokens(account.account_id)
        result = _run_secret_tool(
            ["store", "--label", "Nivora Launcher Microsoft", *_account_secret_attributes(account.account_id)],
            input_text=payload,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise AuthenticationError("Не удалось сохранить Microsoft-токен в Secret Service") from exc
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip().replace("\n", " ")[:240]
        suffix = f": {detail}" if detail else ""
        raise AuthenticationError(f"Secret Service не сохранил Microsoft-токен{suffix}")


def _clear_account_tokens(account_id: str) -> None:
    if os.name == "nt":
        _account_secret_attributes(account_id)
        for service in (WINDOWS_CREDENTIAL_SERVICE, WINDOWS_LEGACY_CREDENTIAL_SERVICE):
            _windows_delete_chunked(f"{service}:{account_id}")
        _windows_credential_delete(f"{account_id}@{WINDOWS_CREDENTIAL_SERVICE}")
        if _windows_credential_read(WINDOWS_CREDENTIAL_SERVICE, expected_user=account_id) is not None:
            _windows_credential_delete(WINDOWS_CREDENTIAL_SERVICE)
        return
    try:
        result = _run_secret_tool(["clear", *_account_secret_attributes(account_id)])
    except (OSError, subprocess.SubprocessError) as exc:
        raise AuthenticationError("Не удалось удалить Microsoft-токен из Secret Service") from exc
    if result.returncode not in {0, 1}:
        detail = (result.stderr or result.stdout or "").strip().replace("\n", " ")[:240]
        suffix = f": {detail}" if detail else ""
        raise AuthenticationError(f"Secret Service не удалил Microsoft-токен{suffix}")


def _run_secret_tool(arguments: list[str], *, input_text: str | None = None):
    command = _secret_tool()
    environment = os.environ.copy()
    result = subprocess.run(
        [command, *arguments],
        input=input_text,
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
        env=environment,
    )
    if result.returncode == 0:
        return result

    # Minimal desktop sessions sometimes have libsecret installed but have
    # not started the Secret Service implementation yet. Start only the
    # secrets component and retry the original operation once.
    keyring = shutil.which("gnome-keyring-daemon")
    if not keyring or keyring == command:
        return result
    try:
        started = subprocess.run(
            [keyring, "--start", "--components=secrets"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
            env=environment,
        )
    except (OSError, subprocess.SubprocessError):
        return result
    if started.returncode != 0:
        return result
    for line in (started.stdout or "").splitlines():
        key, separator, value = line.partition("=")
        if separator and key and value:
            environment[key] = value
            os.environ[key] = value
    return subprocess.run(
        [command, *arguments],
        input=input_text,
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
        env=environment,
    )


# Windows Credential Manager. Формат обязан совпадать с Rust-движком
# (rust_engine/accounts.rs, windows_keyring): вход Microsoft выполняет Rust и
# пишет токен в цель «trial-launcher-microsoft:<account_id>» (UTF-16LE), а игру
# запускает Python. Раньше Python читал через пакет keyring (цели
# «trial-launcher-microsoft» / «<id>@trial-launcher-microsoft»), не видел
# свежий токен и сразу после входа сообщал «Microsoft-токен истёк».
WINDOWS_CREDENTIAL_SERVICE = "trial-launcher-microsoft"
WINDOWS_LEGACY_CREDENTIAL_SERVICE = "nivora-launcher-microsoft"
_CRED_TYPE_GENERIC = 1
_CRED_PERSIST_LOCAL_MACHINE = 2


def _windows_credential_target(account_id: str) -> str:
    return f"{WINDOWS_CREDENTIAL_SERVICE}:{account_id}"


# Диспетчер учётных данных хранит не больше 2560 байт на секрет, а токены
# Minecraft + Microsoft в UTF-16 обычно длиннее — CredWriteW отказывал, и вход
# на Windows не сохранялся. Длинный пейлоад пишется частями «<target>:partN»,
# в основной записи — маркер «nivora-chunked:N» (как в rust_engine/accounts.rs).
_CHUNK_MARKER = "nivora-chunked:"
_CHUNK_CHARS = 1000
_MAX_CHUNKS = 32


def _split_secret(payload: str) -> list[str]:
    if len(payload) <= _CHUNK_CHARS:
        return [payload]
    return [payload[index : index + _CHUNK_CHARS] for index in range(0, len(payload), _CHUNK_CHARS)]


def _chunk_count(value: str | None) -> int | None:
    if not value or not value.startswith(_CHUNK_MARKER):
        return None
    try:
        count = int(value[len(_CHUNK_MARKER):].strip())
    except ValueError:
        return None
    return count if 1 <= count <= _MAX_CHUNKS else None


def _windows_read_chunked(target: str) -> str | None:
    main = _windows_credential_read(target)
    count = _chunk_count(main)
    if count is None:
        return main
    parts: list[str] = []
    for index in range(count):
        part = _windows_credential_read(f"{target}:part{index}")
        if not part:
            return None
        parts.append(part)
    return "".join(parts)


def _windows_delete_chunked(target: str) -> None:
    count = _chunk_count(_windows_credential_read(target))
    for index in range(count or 0):
        _windows_credential_delete(f"{target}:part{index}")
    _windows_credential_delete(target)


def _windows_write_chunked(target: str, username: str, payload: str) -> bool:
    _windows_delete_chunked(target)
    parts = _split_secret(payload)
    if len(parts) == 1:
        return _windows_credential_write(target, username, payload)
    if len(parts) > _MAX_CHUNKS:
        return False
    for index, part in enumerate(parts):
        if not _windows_credential_write(f"{target}:part{index}", f"{username}:part{index}", part):
            return False
    return _windows_credential_write(target, username, f"{_CHUNK_MARKER}{len(parts)}")


def _windows_credential_api():
    import ctypes
    from ctypes import wintypes

    class FILETIME(ctypes.Structure):
        _fields_ = [("dwLowDateTime", wintypes.DWORD), ("dwHighDateTime", wintypes.DWORD)]

    class CREDENTIALW(ctypes.Structure):
        _fields_ = [
            ("Flags", wintypes.DWORD),
            ("Type", wintypes.DWORD),
            ("TargetName", wintypes.LPWSTR),
            ("Comment", wintypes.LPWSTR),
            ("LastWritten", FILETIME),
            ("CredentialBlobSize", wintypes.DWORD),
            ("CredentialBlob", ctypes.POINTER(ctypes.c_ubyte)),
            ("Persist", wintypes.DWORD),
            ("AttributeCount", wintypes.DWORD),
            ("Attributes", ctypes.c_void_p),
            ("TargetAlias", wintypes.LPWSTR),
            ("UserName", wintypes.LPWSTR),
        ]

    advapi = ctypes.WinDLL("advapi32", use_last_error=True)
    advapi.CredReadW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.POINTER(ctypes.POINTER(CREDENTIALW))]
    advapi.CredReadW.restype = wintypes.BOOL
    advapi.CredWriteW.argtypes = [ctypes.POINTER(CREDENTIALW), wintypes.DWORD]
    advapi.CredWriteW.restype = wintypes.BOOL
    advapi.CredDeleteW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD]
    advapi.CredDeleteW.restype = wintypes.BOOL
    advapi.CredFree.argtypes = [ctypes.c_void_p]
    advapi.CredFree.restype = None
    return ctypes, advapi, CREDENTIALW


def _windows_credential_read(target: str, *, expected_user: str | None = None) -> str | None:
    try:
        ctypes, advapi, CREDENTIALW = _windows_credential_api()
    except (ImportError, OSError, AttributeError):
        return None
    pointer = ctypes.POINTER(CREDENTIALW)()
    if not advapi.CredReadW(target, _CRED_TYPE_GENERIC, 0, ctypes.byref(pointer)):
        return None
    try:
        credential = pointer.contents
        if expected_user is not None and (credential.UserName or "") != expected_user:
            return None
        size = int(credential.CredentialBlobSize)
        blob = ctypes.string_at(credential.CredentialBlob, size) if size else b""
    finally:
        advapi.CredFree(pointer)
    try:
        return blob.decode("utf-16-le")
    except UnicodeDecodeError:
        return None


def _windows_credential_write(target: str, username: str, payload: str) -> bool:
    try:
        ctypes, advapi, CREDENTIALW = _windows_credential_api()
    except (ImportError, OSError, AttributeError):
        return False
    data = payload.encode("utf-16-le")
    buffer = (ctypes.c_ubyte * max(1, len(data))).from_buffer_copy(data or b"\0")
    credential = CREDENTIALW()
    credential.Type = _CRED_TYPE_GENERIC
    credential.TargetName = target
    credential.CredentialBlobSize = len(data)
    credential.CredentialBlob = ctypes.cast(buffer, ctypes.POINTER(ctypes.c_ubyte))
    credential.Persist = _CRED_PERSIST_LOCAL_MACHINE
    credential.UserName = username
    return bool(advapi.CredWriteW(ctypes.byref(credential), 0))


def _windows_credential_delete(target: str) -> None:
    try:
        _ctypes, advapi, _credential = _windows_credential_api()
    except (ImportError, OSError, AttributeError):
        return
    advapi.CredDeleteW(target, _CRED_TYPE_GENERIC, 0)


def _windows_load_token_payload(account_id: str) -> str:
    _account_secret_attributes(account_id)
    target = _windows_credential_target(account_id)
    payload = _windows_read_chunked(target)
    if payload:
        return payload
    # Старые места хранения: имя сервиса переходного периода (Rust) и формат
    # пакета keyring (прежний Python). Найденный токен переносим в общий формат.
    legacy = (
        _windows_credential_read(f"{WINDOWS_LEGACY_CREDENTIAL_SERVICE}:{account_id}")
        or _windows_credential_read(f"{account_id}@{WINDOWS_CREDENTIAL_SERVICE}")
        or _windows_credential_read(WINDOWS_CREDENTIAL_SERVICE, expected_user=account_id)
    )
    if not legacy:
        try:
            legacy = _windows_keyring().get_password(WINDOWS_CREDENTIAL_SERVICE, account_id) or ""
        except Exception:
            legacy = ""
    if legacy and _windows_write_chunked(target, account_id, legacy):
        _windows_credential_delete(f"{WINDOWS_LEGACY_CREDENTIAL_SERVICE}:{account_id}")
    return legacy


def _windows_keyring():
    try:
        import keyring
    except ImportError as exc:
        raise AuthenticationError("Установите вариант Nivora Launcher для Windows с поддержкой диспетчера учётных данных") from exc
    try:
        if keyring.get_keyring().priority <= 0:
            raise AuthenticationError("Диспетчер учётных данных Windows недоступен")
    except AuthenticationError:
        raise
    except Exception as exc:
        raise AuthenticationError("Диспетчер учётных данных Windows недоступен") from exc
    return keyring


def microsoft_login_status(config: ConfigStore) -> dict[str, Any]:
    path = config.paths.cache_home / MSA_STATUS_FILE
    if path.is_symlink() or not path.exists():
        return {"status": "idle"}
    try:
        if path.stat().st_size > 256 * 1024:
            return {"status": "idle"}
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError):
        return {"status": "idle"}
    return value if isinstance(value, dict) else {"status": "idle"}


def _write_microsoft_status(config: ConfigStore, value: dict[str, Any]) -> None:
    _write_private_json(
        config.paths.cache_home / MSA_STATUS_FILE,
        {**value, "updated_at": time.time()},
    )
