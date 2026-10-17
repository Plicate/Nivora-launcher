from __future__ import annotations

import json
import os
import shlex
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from scripts import arch_native_build as build


class ArchNativeBuildTests(unittest.TestCase):
    def test_native_flags_drop_lto_and_keep_hardening(self) -> None:
        result = shlex.split(build.native_flags(
            '-O2 -pipe -fstack-protector-strong -D_FORTIFY_SOURCE=2 '
            '-flto=auto -fno-fat-lto-objects -fuse-linker-plugin '
            '-fwhole-program -fuse-ld=lld -ffat-lto-objects'
        ))
        self.assertEqual(result, ['-O2', '-pipe', '-fstack-protector-strong', '-D_FORTIFY_SOURCE=2', '-fno-lto'])
        self.assertEqual(shlex.split(build.native_flags('-Wl,-z,relro -fuse-ld=mold', linking=True)),
                         ['-Wl,-z,relro', '-fno-lto', '-fuse-ld=bfd'])

    def test_target_specific_and_host_flags_cannot_reenable_native_lto(self) -> None:
        source = {
            'CFLAGS': '-O2 -flto',
            'CFLAGS_x86_64_unknown_linux_gnu': '-O3 -flto=thin',
            'CFLAGS_x86_64-unknown-linux-gnu': '-O2 -flto=auto',
            'HOST_CFLAGS': '-flto=auto',
            'TARGET_CXXFLAGS': '-flto=thin',
            'LDFLAGS': '-Wl,-z,now -fuse-ld=lld',
            'CC': 'clang',
            'CC_x86_64-unknown-linux-gnu': 'clang',
            'HOST_CXX': 'clang++',
            'AR_x86_64_unknown_linux_gnu': 'llvm-ar',
            'RANLIB': 'llvm-ranlib',
            'ARFLAGS': 'rcsT',
            'ARFLAGS_x86_64_unknown_linux_gnu': 'rcsT',
            'HOME': '/test/home',
        }
        original = source.copy()
        env = build.build_environment(source)
        self.assertEqual(source, original)
        for name in ['CFLAGS', 'CFLAGS_x86_64_unknown_linux_gnu', 'CFLAGS_x86_64-unknown-linux-gnu',
                     'HOST_CFLAGS', 'TARGET_CXXFLAGS', 'LDFLAGS']:
            flags = shlex.split(env[name])
            self.assertIn('-fno-lto', flags)
            self.assertFalse(any(flag.startswith('-flto') for flag in flags))
            self.assertNotIn('-fuse-ld=lld', flags)
        self.assertEqual(env['CC_x86_64-unknown-linux-gnu'], 'gcc')
        self.assertEqual(env['HOST_CXX'], 'g++')
        self.assertEqual(env['AR_x86_64_unknown_linux_gnu'], 'ar')
        self.assertEqual(env['RANLIB'], 'ranlib')
        self.assertNotIn('ARFLAGS', env)
        self.assertNotIn('ARFLAGS_x86_64_unknown_linux_gnu', env)
        self.assertEqual(env['HOME'], source['HOME'])

    def test_rust_codegen_overrides_select_one_linker_without_dropping_other_flags(self) -> None:
        flags = build.rust_flags([
            '-C', 'linker=clang', '--codegen=linker-flavor=gnu-lld-cc',
            '-Clinker-features=+lld', '-Clto=thin', '-C', 'linker-plugin-lto=yes',
            '-C', 'target-cpu=x86-64-v3', '-Clink-arg=-fuse-ld=lld',
            '-C', 'link-args=-fuse-ld=mold -Wl,-z,relro', '--cfg', 'trial_package',
        ])
        self.assertIn('target-cpu=x86-64-v3', flags)
        self.assertIn('link-args=-Wl,-z,relro', flags)
        self.assertIn('--cfg', flags)
        self.assertIn('trial_package', flags)
        self.assertEqual(flags.count('linker=gcc'), 1)
        self.assertEqual(flags.count('link-arg=-fuse-ld=bfd'), 1)
        self.assertIn('linker-flavor=gcc', flags)
        self.assertIn('lto=off', flags)
        self.assertIn('linker-plugin-lto=no', flags)
        self.assertFalse(any('lld' in flag or 'mold' in flag for flag in flags))

    def test_encoded_rustflags_have_precedence_and_target_overrides_are_removed(self) -> None:
        env = build.build_environment({
            'RUSTFLAGS': '-C target-cpu=native -C lto=thin',
            'CARGO_ENCODED_RUSTFLAGS': '-C\x1ftarget-cpu=x86-64-v2\x1f-Clink-arg=-fuse-ld=lld',
            'CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS': '-C lto=thin',
            'CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER': 'clang',
            'CARGO_PROFILE_RELEASE_LTO': 'thin',
        })
        flags = env['CARGO_ENCODED_RUSTFLAGS'].split('\x1f')
        self.assertIn('target-cpu=x86-64-v2', flags)
        self.assertNotIn('target-cpu=native', flags)
        self.assertEqual(shlex.split(env['RUSTFLAGS']), flags)
        self.assertNotIn('CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS', env)
        self.assertEqual(env['CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER'], 'gcc')
        self.assertEqual(env['CARGO_PROFILE_RELEASE_LTO'], 'false')

    def test_empty_encoded_rustflags_still_override_plain_rustflags(self) -> None:
        env = build.build_environment({'CARGO_ENCODED_RUSTFLAGS': '', 'RUSTFLAGS': '--cfg should_not_be_used'})
        self.assertNotIn('should_not_be_used', env['CARGO_ENCODED_RUSTFLAGS'])

    def test_environment_normalization_is_idempotent(self) -> None:
        once = build.build_environment({'CFLAGS': '-O2 -flto', 'RUSTFLAGS': '-C target-cpu=x86-64-v3'})
        self.assertEqual(build.build_environment(once), once)

    def test_malformed_flags_fail_instead_of_silently_changing_the_build(self) -> None:
        with self.assertRaises(ValueError):
            build.build_environment({'CFLAGS': '-DNAME="unterminated'})
        with self.assertRaises(ValueError):
            build.rust_flags(['-C'])

    def test_main_passes_normalized_environment_and_preserves_failure_status(self) -> None:
        with (
            patch.object(build.sys, 'platform', 'linux'),
            patch.object(build.platform, 'machine', return_value='x86_64'),
            patch.object(build.shutil, 'which', side_effect=lambda tool: '/usr/bin/' + tool),
            patch.dict(os.environ, {'CFLAGS': '-flto=thin', 'RUSTFLAGS': '-C link-arg=-fuse-ld=lld'}, clear=True),
            patch.object(build.subprocess, 'run', return_value=SimpleNamespace(returncode=19)) as run,
        ):
            self.assertEqual(build.main(), 19)
        args, kwargs = run.call_args
        self.assertEqual(args[0], [
            'npm', 'run', 'desktop:build', '--',
            '--no-bundle', '--config', 'src-tauri/tauri.arch.conf.json',
        ])
        self.assertEqual(kwargs['cwd'], Path(__file__).resolve().parents[1] / 'frontend')
        self.assertEqual(kwargs['env']['CARGO_TARGET_DIR'], str(kwargs['cwd'] / 'src-tauri' / 'target'))
        self.assertEqual(kwargs['env']['CFLAGS'], '-fno-lto')
        self.assertNotIn('lld', kwargs['env']['CARGO_ENCODED_RUSTFLAGS'])
        self.assertNotIn('TAURI_SIGNING_PRIVATE_KEY', kwargs['env'])

    def test_arch_override_disables_artifact_signing_not_update_verification(self) -> None:
        directory = Path(__file__).resolve().parents[1] / 'frontend' / 'src-tauri'
        base = json.loads((directory / 'tauri.conf.json').read_text())
        linux = json.loads((directory / 'tauri.linux.conf.json').read_text())
        arch = json.loads((directory / 'tauri.arch.conf.json').read_text())
        self.assertEqual(arch, {
            '$schema': 'https://schema.tauri.app/config/2',
            'bundle': {'active': False, 'createUpdaterArtifacts': False},
        })
        self.assertTrue(base['bundle']['active'])
        # Самообновление настроено: фид — latest.json из GitHub Releases,
        # публичный ключ вшит в конфиг; updater-артефакты включены в базовой
        # сборке (подпись кладётся через TAURI_SIGNING_PRIVATE_KEY в CI).
        # Arch-оверрайд остаётся без updater-артефактов: pacman-пакет
        # обновляется своей системой, подписывать ему нечего.
        self.assertTrue(base['bundle']['createUpdaterArtifacts'])
        updater = base.get('plugins', {}).get('updater', {})
        self.assertEqual(len(updater.get('endpoints', [])), 1)
        self.assertIn('releases/latest/download/latest.json', updater['endpoints'][0])
        self.assertTrue(updater.get('pubkey'))
        self.assertNotIn('plugins', arch)
        self.assertNotIn('app', arch)
        # Platform backend resources survive the bundle-only override.
        merged_bundle = {**base['bundle'], **linux['bundle'], **arch['bundle']}
        self.assertFalse(merged_bundle['active'])
        self.assertFalse(merged_bundle['createUpdaterArtifacts'])
        self.assertEqual(merged_bundle['resources'], linux['bundle']['resources'])

    def test_main_reports_missing_tools_before_launching_npm(self) -> None:
        with (
            patch.object(build.sys, 'platform', 'linux'),
            patch.object(build.platform, 'machine', return_value='x86_64'),
            patch.object(build.shutil, 'which', return_value=None),
            patch.object(build.subprocess, 'run') as run,
        ):
            self.assertEqual(build.main(), 1)
            run.assert_not_called()

    def test_pkgbuild_uses_the_compatibility_profile_and_disables_makepkg_lto(self) -> None:
        text = (Path(__file__).resolve().parents[1] / 'packaging/arch/PKGBUILD').read_text()
        self.assertIn("options=('!lto')", text)
        self.assertIn('python scripts/arch_native_build.py || return $?', text)
        self.assertNotIn('RUSTFLAGS="${RUSTFLAGS} -C link-arg=-fuse-ld=bfd"', text)

    @unittest.skipUnless(all(shutil.which(tool) for tool in ['gcc', 'ar', 'nm', 'objdump', 'ld.bfd']), 'GCC/binutils are required')
    def test_sanitized_native_objects_export_symbols_and_link_with_bfd(self) -> None:
        env = build.build_environment({**os.environ, 'CFLAGS': '-O2 -flto=auto -fno-fat-lto-objects',
                                      'LDFLAGS': '-fuse-ld=lld'})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'native.c').write_text('int native_answer(void) { return 42; }\n')
            (root / 'main.c').write_text('int native_answer(void); int main(void) { return native_answer() == 42 ? 0 : 1; }\n')
            def execute(arguments: list[str]) -> subprocess.CompletedProcess[str]:
                return subprocess.run(arguments, cwd=root, env=env, check=True, capture_output=True, text=True, timeout=30)
            execute(['gcc', *shlex.split(env['CFLAGS']), '-c', 'native.c', '-o', 'native.o'])
            execute(['ar', 'rcs', 'libnative.a', 'native.o'])
            symbols = execute(['nm', '--defined-only', 'libnative.a']).stdout
            self.assertRegex(symbols, r'\bT native_answer\b')
            sections = execute(['objdump', '-h', 'native.o']).stdout
            self.assertNotIn('.gnu.lto_', sections)
            execute(['gcc', 'main.c', 'libnative.a', *shlex.split(env['LDFLAGS']), '-o', 'native-smoke'])
            execute([str(root / 'native-smoke')])


if __name__ == '__main__':
    unittest.main()
