import { computed, ref, type Ref } from 'vue'
import { bridge } from './bridge'
import { pickFile } from './fileBrowser'
import type { AccountInfo, SkinInfo, SkinProfile } from '../types'

type SkinModel = 'classic' | 'slim'

type SkinWorkflowDependencies = {
  accountMenuId: Ref<string>
  showToast: (message: string) => void
  errorMessage: (error: unknown) => string
}

export function createSkinWorkflow({ accountMenuId, showToast, errorMessage }: SkinWorkflowDependencies) {
  const skinResults = ref<SkinInfo[]>([])
  const skinPanelOpen = ref(false)
  const skinAccountId = ref('')
  const skinProfileData = ref<SkinProfile>()
  const skinProfileLoading = ref(false)
  const skinProfileError = ref('')
  const selectedSkinId = ref('')
  const skinModel = ref<SkinModel>('classic')
  const skinElytraPreview = ref(false)
  const currentSkinModel = ref<SkinModel>('classic')
  const selectedCapeId = ref('')
  const skinEditorOpen = ref(false)
  const capeActionBusy = ref(false)
  let skinProfileRequestId = 0

  const activeRemoteSkin = computed(() => skinProfileData.value?.skins?.find((skin) => skin.state?.toUpperCase() === 'ACTIVE') || skinProfileData.value?.skins?.[0])
  const selectedLocalSkin = computed(() => skinResults.value.find((skin) => skin.skin_id === selectedSkinId.value))
  const skinPreviewImage = computed(() => selectedLocalSkin.value?.image || activeRemoteSkin.value?.url || '')
  const activeCape = computed(() => skinProfileData.value?.capes?.find((cape) => cape.state?.toUpperCase() === 'ACTIVE') || skinProfileData.value?.capes?.[0])
  const availableCapes = computed(() => {
    const capes = skinProfileData.value?.capes || []
    const active = capes.find((cape) => cape.id ? cape.id === activeCape.value?.id : cape === activeCape.value)
    return active ? [active, ...capes.filter((cape) => cape !== active)] : capes
  })
  const selectedCape = computed(() => availableCapes.value.find((cape) => cape.id === selectedCapeId.value) || (!selectedCapeId.value ? activeCape.value : undefined))

  function selectSkinModel(value: SkinModel) {
    skinModel.value = value
    currentSkinModel.value = value
  }

  function selectSkin(skin: SkinInfo) {
    selectedSkinId.value = skin.skin_id
    const model = skin.model === 'slim' ? 'slim' : 'classic'
    skinModel.value = model
    currentSkinModel.value = model
  }

  function selectCurrentSkin() {
    selectedSkinId.value = ''
    const model = activeRemoteSkin.value?.variant?.toUpperCase() === 'SLIM' ? 'slim' : 'classic'
    skinModel.value = model
    currentSkinModel.value = model
  }

  async function loadSkins() {
    try {
      skinResults.value = (await bridge.skinsList()).skins
    } catch (error) {
      showToast(errorMessage(error))
    }
  }

  async function loadSkinProfile(accountId = skinAccountId.value) {
    if (!accountId) return
    const requestId = ++skinProfileRequestId
    skinProfileLoading.value = true
    skinProfileError.value = ''
    try {
      const profile = await bridge.skinProfile(accountId)
      if (requestId !== skinProfileRequestId || !skinPanelOpen.value || skinAccountId.value !== accountId) return
      skinProfileData.value = profile
      selectCurrentSkin()
      selectedCapeId.value = activeCape.value?.id || ''
    } catch (error) {
      if (requestId !== skinProfileRequestId || !skinPanelOpen.value || skinAccountId.value !== accountId) return
      skinProfileData.value = undefined
      skinProfileError.value = errorMessage(error)
      showToast(`Не удалось загрузить профиль Minecraft: ${skinProfileError.value}`)
    } finally {
      if (requestId === skinProfileRequestId) skinProfileLoading.value = false
    }
  }

  async function openSkinSettings(account: AccountInfo) {
    if (account.kind !== 'microsoft') {
      showToast('Настройка скина доступна только для Microsoft-аккаунта')
      return
    }
    accountMenuId.value = ''
    skinAccountId.value = account.account_id
    skinProfileData.value = undefined
    skinProfileError.value = ''
    skinProfileLoading.value = false
    selectedSkinId.value = ''
    selectedCapeId.value = ''
    skinModel.value = 'classic'
    currentSkinModel.value = 'classic'
    skinPanelOpen.value = true
    void loadSkins()
    await loadSkinProfile(account.account_id)
  }

  function closeSkinSettings() {
    skinProfileRequestId += 1
    skinPanelOpen.value = false
    skinProfileData.value = undefined
    skinProfileLoading.value = false
    skinProfileError.value = ''
    selectedSkinId.value = ''
    selectedCapeId.value = ''
    currentSkinModel.value = 'classic'
    skinEditorOpen.value = false
  }

  function openSkinEditor() {
    if (!skinAccountId.value) return
    skinEditorOpen.value = true
  }

  async function onSkinEditorSaved() {
    await loadSkins()
  }

  async function onSkinEditorApplied() {
    await loadSkins()
    if (skinAccountId.value) await loadSkinProfile(skinAccountId.value)
  }

  async function applySelectedSkin(skin: SkinInfo) {
    if (!skinAccountId.value) return
    try {
      await bridge.applySkin(skinAccountId.value, skin.skin_id, skinModel.value)
      showToast(`Скин «${skin.name}» установлен`)
      skinProfileData.value = await bridge.skinProfile(skinAccountId.value)
      selectCurrentSkin()
    } catch (error) {
      showToast(errorMessage(error))
    }
  }

  async function resetSkin() {
    if (!skinAccountId.value) return
    try {
      await bridge.clearSkin(skinAccountId.value)
      showToast('Скин сброшен на стандартный')
      skinProfileData.value = await bridge.skinProfile(skinAccountId.value)
      selectCurrentSkin()
    } catch (error) {
      showToast(errorMessage(error))
    }
  }

  async function selectCape(capeId: string) {
    if (!skinAccountId.value || capeActionBusy.value) return
    if (capeId && !availableCapes.value.some((cape) => cape.id === capeId)) {
      showToast('Можно выбрать только плащ из профиля Minecraft')
      return
    }
    capeActionBusy.value = true
    try {
      if (capeId) await bridge.applyCape(skinAccountId.value, capeId)
      else await bridge.clearCape(skinAccountId.value)
      selectedCapeId.value = capeId
      showToast(capeId ? 'Плащ установлен' : 'Плащ снят')
      skinProfileData.value = await bridge.skinProfile(skinAccountId.value)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      capeActionBusy.value = false
    }
  }

  async function chooseSkinFile() {
    const file = await pickFile({ accept: 'image/png', title: 'PNG-файл скина' })
    if (!file) return
    try {
      const result = await bridge.skinsImport(file.fileName, file.data, skinModel.value)
      skinResults.value.push(result.skin)
      selectSkin(result.skin)
      showToast('Скин добавлен в библиотеку')
    } catch (error) {
      showToast(errorMessage(error))
    }
  }

  return {
    skinResults,
    skinPanelOpen,
    skinAccountId,
    skinProfileData,
    skinProfileLoading,
    skinProfileError,
    selectedSkinId,
    skinModel,
    skinElytraPreview,
    currentSkinModel,
    selectedCapeId,
    skinEditorOpen,
    capeActionBusy,
    activeRemoteSkin,
    selectedLocalSkin,
    skinPreviewImage,
    activeCape,
    availableCapes,
    selectedCape,
    selectSkinModel,
    selectSkin,
    selectCurrentSkin,
    loadSkins,
    loadSkinProfile,
    openSkinSettings,
    closeSkinSettings,
    openSkinEditor,
    onSkinEditorSaved,
    onSkinEditorApplied,
    applySelectedSkin,
    resetSkin,
    selectCape,
    chooseSkinFile,
  }
}
