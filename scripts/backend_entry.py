"""Совместимость со старыми spec-файлами: точка входа живёт в пакете."""

from nivora_launcher.backend_entrypoint import main

if __name__ == "__main__":
    raise SystemExit(main())
