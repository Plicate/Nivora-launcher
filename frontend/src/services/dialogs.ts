import { reactive } from 'vue'

type DialogKind = 'alert' | 'confirm' | 'prompt' | 'choose'

interface DialogChoice {
  value: string
  label: string
  description?: string
}

interface DialogOptions {
  title?: string
  detail?: string
  confirmLabel?: string
  cancelLabel?: string
  placeholder?: string
  initialValue?: string
  choices?: DialogChoice[]
  danger?: boolean
}

interface DialogRequest extends DialogOptions {
  id: number
  kind: DialogKind
  message: string
  resolve: (value: boolean | string | undefined) => void
}

export const dialogQueue = reactive<DialogRequest[]>([])
let nextDialogId = 1

function enqueue(kind: DialogKind, message: string, options: DialogOptions, signal?: AbortSignal): Promise<boolean | string | undefined> {
  return new Promise((resolve) => {
    const id = nextDialogId++
    let settled = false
    const finish = (value: boolean | string | undefined) => {
      if (settled) return
      settled = true
      signal?.removeEventListener('abort', abort)
      resolve(value)
    }
    const abort = () => {
      const index = dialogQueue.findIndex((request) => request.id === id)
      if (index >= 0) dialogQueue.splice(index, 1)
      finish(undefined)
    }
    if (signal?.aborted) {
      finish(undefined)
      return
    }
    signal?.addEventListener('abort', abort, { once: true })
    dialogQueue.push({ id, kind, message, resolve: finish, ...options })
  })
}

export function alertDialog(message: string, options: DialogOptions = {}, signal?: AbortSignal): Promise<void> {
  return enqueue('alert', message, options, signal).then(() => undefined)
}

export function confirmDialog(message: string, options: DialogOptions = {}, signal?: AbortSignal): Promise<boolean> {
  return enqueue('confirm', message, options, signal).then((value) => value === true)
}

export function promptDialog(message: string, options: DialogOptions = {}, signal?: AbortSignal): Promise<string | undefined> {
  return enqueue('prompt', message, options, signal).then((value) => (typeof value === 'string' ? value : undefined))
}

export function chooseDialog(message: string, choices: DialogChoice[], options: DialogOptions = {}, signal?: AbortSignal): Promise<string | undefined> {
  if (!choices.length) return Promise.resolve(undefined)
  return enqueue('choose', message, { ...options, choices }, signal).then((value) => (typeof value === 'string' ? value : undefined))
}

export function resolveDialog(value: boolean | string | undefined) {
  const request = dialogQueue.shift()
  request?.resolve(value)
}
