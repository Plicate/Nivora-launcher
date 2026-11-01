import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

const dialog = readFileSync(new URL('../src/components/GlobalSearchDialog.vue', import.meta.url), 'utf8')

function section(start, end) {
  const startIndex = dialog.indexOf(start)
  const endIndex = dialog.indexOf(end, startIndex + start.length)
  assert.ok(startIndex >= 0 && endIndex > startIndex, `${start} section boundaries are present`)
  return dialog.slice(startIndex, endIndex)
}

test('global search cancels its timer and aborts an active request', () => {
  const cancelSearch = section('function cancelSearch()', '\nfunction scheduleSearch()')
  assert.match(cancelSearch, /window\.clearTimeout\(searchTimer\)/)
  assert.match(cancelSearch, /activeSearch\?\.abort\(\)/)
  assert.match(cancelSearch, /searchGeneration \+= 1/)
})

test('query changes and short queries invalidate older searches', () => {
  const scheduleSearch = section('function scheduleSearch()', '\nfunction closeDialog()')
  assert.ok(scheduleSearch.indexOf('cancelSearch()') < scheduleSearch.indexOf('trimmed.length < 2'))
  assert.match(scheduleSearch, /trimmed\.length < 2[\s\S]*?response\.value = null[\s\S]*?searching\.value = false/)
  assert.match(scheduleSearch, /bridge\.globalSearch\(trimmed, controller\.signal\)/)
  assert.match(scheduleSearch, /generation === searchGeneration && !controller\.signal\.aborted\) response\.value = nextResponse/)
  assert.match(scheduleSearch, /catch \{\s+if \(generation === searchGeneration && !controller\.signal\.aborted\) response\.value = null/)
  assert.match(scheduleSearch, /finally \{\s+if \(generation === searchGeneration\)/)
  assert.match(dialog, /watch\(query, scheduleSearch, \{ flush: 'sync' \}\)/)
})

test('closing or unmounting cancels pending global search work', () => {
  const closeDialog = section('function closeDialog()', '\nfunction choose(')
  assert.match(closeDialog, /cancelSearch\(\)/)
  const unmount = section('onBeforeUnmount(() => {', '\n})')
  assert.match(unmount, /cancelSearch\(\)/)
  assert.match(dialog, /@click\.self="closeDialog"/)
  assert.match(dialog, /aria-label="Закрыть" @click="closeDialog"/)
  assert.match(dialog, /function onKeydown\([\s\S]*?if \(event\.key === 'Escape'\)[\s\S]*?closeDialog\(\)/)
})
