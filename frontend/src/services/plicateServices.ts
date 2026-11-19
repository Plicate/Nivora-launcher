import { ref, watch } from 'vue'

const STORAGE_KEY = 'nivora-plicate-services'

function readInitial(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) !== '0'
  } catch {
    return true
  }
}

export const plicateServicesEnabled = ref(readInitial())

watch(plicateServicesEnabled, (value) => {
  try {
    localStorage.setItem(STORAGE_KEY, value ? '1' : '0')
  } catch { /* storage may be disabled */ }
})

export function setPlicateServicesEnabled(value: boolean) {
  plicateServicesEnabled.value = value
}
