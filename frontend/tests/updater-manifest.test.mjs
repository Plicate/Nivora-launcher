import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const writer = fileURLToPath(new URL('../../scripts/write_updater_manifest.mjs', import.meta.url))

test('updater manifest is valid JSON and escapes signatures and release paths', () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'nivora-updater-manifest-'))
  const output = path.join(directory, 'latest.json')
  const signature = 'sig"\\line\nnext'
  try {
    execFileSync(process.execPath, [
      writer,
      output,
      '2.0.1',
      'Plicate/Nivora-launcher',
      'v2 "stable"',
      'Nivora Launcher "stable".exe',
      signature,
      '2026-10-28T00:00:00Z',
    ])
    const manifest = JSON.parse(readFileSync(output, 'utf8'))
    assert.equal(manifest.version, '2.0.1')
    assert.equal(manifest.notes, 'Nivora Launcher 2.0.1')
    assert.equal(manifest.pub_date, '2026-10-28T00:00:00Z')
    for (const platform of ['windows-x86_64', 'windows-x86_64-nsis']) {
      assert.equal(manifest.platforms[platform].signature, signature)
      assert.equal(
        manifest.platforms[platform].url,
        'https://github.com/Plicate/Nivora-launcher/releases/download/v2%20%22stable%22/Nivora%20Launcher%20%22stable%22.exe',
      )
    }
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
