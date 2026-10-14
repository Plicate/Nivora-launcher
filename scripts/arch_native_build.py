#!/usr/bin/env python3
"""Run the Arch x86_64 desktop build with native ELF objects, not C/C++ LTO.

GCC/Clang LTO objects can lose their native symbol index when bundled into Rust
rlibs. Switching only the final linker cannot repair such objects. This build
profile removes inherited native LTO flags and selects one GCC/BFD toolchain.
It does not modify system makepkg/Cargo configuration or crypto dependencies.
"""
from __future__ import annotations

import os
import platform
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Mapping

TARGET = "x86_64_unknown_linux_gnu"
RUST_CODEGEN_OVERRIDES = {
    "linker", "linker-flavor", "linker-features", "lto", "linker-plugin-lto",
}


def native_flags(value: str, *, linking: bool = False) -> str:
    flags = [
        flag for flag in shlex.split(value)
        if not flag.startswith(("-flto", "-fuse-ld="))
        and flag not in {
            "-fno-lto", "-fuse-linker-plugin", "-fno-fat-lto-objects",
            "-ffat-lto-objects", "-fwhole-program",
        }
    ]
    flags.append("-fno-lto")
    if linking:
        flags.append("-fuse-ld=bfd")
    return shlex.join(flags)


def rust_flags(flags: list[str]) -> list[str]:
    result: list[str] = []
    index = 0
    while index < len(flags):
        flag = flags[index]
        index += 1
        if flag in {"-C", "--codegen"}:
            if index == len(flags):
                raise ValueError(f"Missing value for Rust compiler flag {flag}")
            setting = flags[index]
            index += 1
        elif flag.startswith("-C") and len(flag) > 2:
            setting = flag[2:]
        elif flag.startswith("--codegen="):
            setting = flag[len("--codegen="):]
        else:
            result.append(flag)
            continue
        name, _, value = setting.partition("=")
        if name in RUST_CODEGEN_OVERRIDES:
            continue
        if name in {"link-arg", "link-args"}:
            if name == "link-arg" and value.startswith(("-fuse-ld=", "-flto")):
                continue
            if name == "link-args":
                args = [arg for arg in shlex.split(value) if not arg.startswith(("-fuse-ld=", "-flto"))]
                if not args:
                    continue
                setting = "link-args=" + shlex.join(args)
        result.extend(["-C", setting])
    result.extend([
        "-C", "linker=gcc",
        "-C", "linker-flavor=gcc",
        "-C", "link-arg=-fuse-ld=bfd",
        "-C", "lto=off",
        "-C", "linker-plugin-lto=no",
    ])
    return result


def build_environment(source: Mapping[str, str]) -> dict[str, str]:
    env = dict(source)
    # cc-rs checks target-specific and HOST_/TARGET_ variables before globals.
    # Normalize all spellings, including target triples containing hyphens.
    for name in list(env):
        for tool, executable in {"CC": "gcc", "CXX": "g++", "AR": "ar", "RANLIB": "ranlib"}.items():
            if name in {tool, "HOST_" + tool, "TARGET_" + tool} or name.startswith(tool + "_"):
                env[name] = executable
        if name in {"ARFLAGS", "HOST_ARFLAGS", "TARGET_ARFLAGS"} or name.startswith("ARFLAGS_"):
            del env[name]
            continue
        for family in ("CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS"):
            if name in {family, "HOST_" + family, "TARGET_" + family} or name.startswith(family + "_"):
                env[name] = native_flags(env[name], linking=family == "LDFLAGS")
    for tool, executable in {"CC": "gcc", "CXX": "g++", "AR": "ar", "RANLIB": "ranlib"}.items():
        env[tool] = executable
        env[tool + "_" + TARGET] = executable
    for family in ("CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS"):
        env[family] = native_flags(env.get(family, ""), linking=family == "LDFLAGS")
    # Cargo gives encoded flags precedence over RUSTFLAGS and target config.
    encoded = source.get("CARGO_ENCODED_RUSTFLAGS")
    if encoded is not None:
        inherited = encoded.split("\x1f") if encoded else []
    else:
        inherited = shlex.split(source.get("RUSTFLAGS", ""))
    normalized = rust_flags(inherited)
    for name in list(env):
        if name.startswith("CARGO_TARGET_") and name.endswith("_RUSTFLAGS"):
            del env[name]
    env["RUSTFLAGS"] = shlex.join(normalized)
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(normalized)
    env["CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER"] = "gcc"
    env["CARGO_PROFILE_RELEASE_LTO"] = "false"
    return env


def main() -> int:
    if sys.platform != "linux" or platform.machine().lower() not in {"x86_64", "amd64"}:
        print("This compatibility build profile supports Arch Linux x86_64 only.", file=sys.stderr)
        return 1
    for tool in ("gcc", "g++", "ar", "ranlib", "ld.bfd", "cargo", "npm"):
        if shutil.which(tool) is None:
            print(f"Missing required build tool: {tool}", file=sys.stderr)
            return 1
    try:
        env = build_environment(os.environ)
    except ValueError as error:
        print(f"Invalid inherited compiler flags: {error}", file=sys.stderr)
        return 1
    print("Arch desktop build: GCC/BFD, native LTO disabled, Rust linker-plugin LTO disabled.", flush=True)
    frontend = Path(__file__).resolve().parents[1] / "frontend"
    # Keep artifacts inside makepkg's clean source tree and at package()'s path.
    env["CARGO_TARGET_DIR"] = str(frontend / "src-tauri" / "target")
    # makepkg packages this binary itself. Do not generate deb/rpm/AppImage or
    # signed updater artifacts; local Arch builds must not need a private key.
    command = [
        "npm", "run", "desktop:build", "--",
        "--no-bundle", "--config", "src-tauri/tauri.arch.conf.json",
    ]
    try:
        result = subprocess.run(command, cwd=frontend, env=env, check=False)
    except OSError as error:
        print(f"Unable to launch desktop build: {error}", file=sys.stderr)
        return 1
    return result.returncode if result.returncode >= 0 else 128 - result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
