<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import type { AccountInfo } from '../types'
import type { PluginUiTone } from '../../../plugins/api'
import Icon from './Icon.vue'
import SkinPreview from './SkinPreview.vue'
import SkinEditorDialog from './SkinEditorDialog.vue'
import SkinThumb3D from './SkinThumb3D.vue'
import CapeThumb from './CapeThumb.vue'
import { openContextMenu } from '../services/contextMenu'
import { createSkinWorkflow } from '../services/skinWorkflow'
import { bridge } from '../services/bridge'
import { nicknameError } from '../services/nickname'

const props = defineProps<{
  accounts: AccountInfo[]
  hasClientId: boolean
  currentView: string
  busy: boolean
  loginBusy: boolean
  loginEnabled: boolean
  elyLoginBusy: boolean
  accountName: string
  accountNameError: string
  confirmDestructive: (message: string) => Promise<boolean>
  refreshBootstrap: () => Promise<void>
  errorMessage: (error: unknown) => string
  showToast: (message: string, tone?: PluginUiTone) => void
  openSkinsFolderBrowser: () => void
}>()

const emit = defineEmits<{
  (event: 'update:accounts', value: AccountInfo[]): void
  (event: 'update:accountName', value: string): void
  (event: 'select-account', value: string): void
  (event: 'create-offline-account'): void
  (event: 'login-microsoft'): void
  (event: 'login-ely'): void
}>()

const accountMenuId = ref('')

function closeAccountMenuOnOutside(event: MouseEvent) {
  if ((event.target as Element).closest?.('.account-menu, .account-actions')) return
  accountMenuId.value = ''
}

function handleAccountMenuKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && accountMenuId.value) {
    accountMenuId.value = ''
    event.stopImmediatePropagation()
  }
}

async function accountAction(account: AccountInfo, action: 'default' | 'up' | 'down' | 'delete') {
  accountMenuId.value = ''
  try {
    if (action === 'default') {
      const result = await bridge.setDefaultAccount(account.account_id)
      emit('update:accounts', result.accounts)
      emit('select-account', 'auto')
      props.showToast(`Профиль «${account.display_name}» выбран по умолчанию`)
    } else if (action === 'up' || action === 'down') {
      emit('update:accounts', (await bridge.moveAccount(account.account_id, action)).accounts)
      props.showToast('Порядок профилей обновлён')
    } else if (await props.confirmDestructive(`Удалить профиль «${account.display_name}»?`)) {
      await bridge.deleteAccount(account.account_id)
      if (skinAccountId.value === account.account_id) closeSkinSettings()
      await props.refreshBootstrap()
      props.showToast('Аккаунт удалён')
    }
  } catch (error) {
    props.showToast(props.errorMessage(error))
  }
}

function accountContextMenu(event: MouseEvent, account: AccountInfo) {
  openContextMenu(event, [
    { id: 'default', label: 'Сделать по умолчанию', icon: 'star', action: () => void accountAction(account, 'default') },
    { id: 'up', label: 'Переместить выше', icon: 'arrow-up', action: () => void accountAction(account, 'up') },
    { id: 'down', label: 'Переместить ниже', icon: 'arrow-down', action: () => void accountAction(account, 'down') },
    { id: 'delete', label: 'Удалить аккаунт', icon: 'trash', danger: true, action: () => void accountAction(account, 'delete') },
  ], account.display_name)
}

const {
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
} = createSkinWorkflow({ accountMenuId, showToast: props.showToast, errorMessage: props.errorMessage })
const skinEditorAccountName = computed(() => props.accounts.find((account) => account.account_id === skinAccountId.value)?.display_name || 'аккаунт')

onMounted(() => {
  window.addEventListener('keydown', handleAccountMenuKeydown)
  document.addEventListener('click', closeAccountMenuOnOutside)
})
onBeforeUnmount(() => {
  window.removeEventListener('keydown', handleAccountMenuKeydown)
  document.removeEventListener('click', closeAccountMenuOnOutside)
})
</script>

<template>
  <section data-page-view="accounts" class="page-content">
    <div class="section-intro"><div><h2>Аккаунты</h2><p>Выбирайте, от чьего имени будет запускаться игра.</p></div><span class="account-login-buttons"><button class="button button--outline" type="button" :disabled="loginBusy || !loginEnabled" @click="emit('login-microsoft')"><Icon name="accounts" :size="17" />Войти через Microsoft</button><button class="button button--outline" type="button" :disabled="elyLoginBusy" @click="emit('login-ely')"><Icon name="accounts" :size="17" />Войти через ely.by</button></span></div>
    <div class="account-layout"><section class="panel account-panel"><div class="panel-heading"><div><h2>Профили</h2></div><span class="count-badge">{{ accounts.length }}</span></div><p v-if="!hasClientId" class="inline-error">Microsoft-вход не настроен в этой сборке. Вводить ID в настройках не нужно; для входа понадобится сборка с настроенным Microsoft OAuth.</p><div v-if="accounts.length" class="account-list"><article v-for="account in accounts" :key="account.account_id" class="account-row" @contextmenu.prevent="accountContextMenu($event, account)"><span class="avatar avatar--blue">{{ account.display_name.slice(0, 1).toUpperCase() }}</span><div class="account-copy"><strong>{{ account.display_name }} <span v-if="account.is_default" class="default-mark">по умолчанию</span></strong><span>{{ account.kind === 'microsoft' ? 'Microsoft account' : account.kind === 'ely' ? 'Аккаунт ely.by' : 'Offline debug account' }}</span></div><span class="account-status"><i />{{ account.kind === 'microsoft' || account.kind === 'ely' ? 'Подключён' : 'Локальный' }}</span><div class="account-actions"><button class="icon-button" type="button" aria-label="Настройки профиля" @click="accountMenuId = accountMenuId === account.account_id ? '' : account.account_id"><Icon name="settings" :size="16" /></button><div v-if="accountMenuId === account.account_id" class="account-menu"><button type="button" :disabled="account.kind !== 'microsoft'" @click="openSkinSettings(account)"><Icon name="skin" :size="14" />Настроить скин</button><button type="button" @click="accountAction(account, 'default')"><Icon name="star" :size="14" />Сделать по умолчанию</button><button type="button" @click="accountAction(account, 'up')"><Icon name="arrow-up" :size="14" />Переместить выше</button><button type="button" @click="accountAction(account, 'down')"><Icon name="arrow-down" :size="14" />Переместить ниже</button><button class="account-menu-danger" type="button" @click="accountAction(account, 'delete')"><Icon name="trash" :size="14" />Удалить аккаунт</button></div></div></article></div><div v-else class="empty-mini empty-mini--roomy"><span class="empty-orb"><Icon name="accounts" :size="19" /></span><strong>Аккаунтов пока нет</strong><span>Создайте локальный аккаунт или войдите через Microsoft.</span></div></section><section class="panel create-account-panel"><h2>Локальный профиль</h2><p>Подходит для одиночного тестирования и разработки модпаков. Онлайн-серверы недоступны.</p><label class="field"><span>Имя игрока <small>3–16 символов: латиница, цифры и _</small></span><input :value="accountName" maxlength="16" placeholder="Player" autocomplete="nickname" spellcheck="false" :aria-invalid="Boolean(accountNameError)" :class="{ 'input--invalid': accountNameError }" @input="emit('update:accountName', ($event.target as HTMLInputElement).value)" @keydown.enter.prevent="emit('create-offline-account')" /></label><p v-if="accountNameError" class="inline-error" role="alert">{{ accountNameError }}</p><button class="button button--accent button--large" type="button" :disabled="busy || Boolean(nicknameError(accountName))" @click="emit('create-offline-account')"><Icon name="plus" :size="18" />Создать offline-профиль</button></section></div>
    <section v-if="skinPanelOpen && skinAccountId" class="panel skin-settings-panel">
      <div class="panel-heading">
        <div><h2>Настройка скина</h2><p>Профиль: {{ accounts.find((account) => account.account_id === skinAccountId)?.display_name }}</p></div>
        <button class="icon-button" type="button" aria-label="Закрыть" @click="closeSkinSettings"><Icon name="close" :size="17" /></button>
      </div>
      <div v-if="skinProfileLoading" class="skin-profile-status" role="status"><span class="skin-profile-spinner" />Загружаю скины и плащи из профиля Minecraft…</div>
      <div v-else-if="skinProfileError" class="skin-profile-status skin-profile-status--error" role="alert"><span>Не удалось получить профиль: {{ skinProfileError }}</span><button class="button button--quiet" type="button" @click="loadSkinProfile()"><Icon name="refresh" :size="14" />Повторить</button></div>
      <div class="skin-settings-layout">
        <div class="skin-preview-3d"><SkinPreview v-if="currentView === 'accounts'" :image="skinPreviewImage" :model="currentSkinModel" :cape="Boolean(selectedCape?.url)" :cape-image="selectedCape?.url || ''" :elytra="skinElytraPreview" /><span class="muted">{{ skinProfileLoading ? 'Загружаю профиль Minecraft…' : selectedLocalSkin ? 'Предпросмотр выбранного скина' : activeRemoteSkin ? 'Текущий скин Minecraft' : 'Активный скин не задан' }} · {{ currentSkinModel === 'slim' ? 'стройные руки' : 'широкие руки' }}</span></div>
        <div class="skin-controls">
          <div class="field"><span>Модель рук</span><div class="segmented-control"><button type="button" :class="{ active: skinModel === 'classic' }" @click="selectSkinModel('classic')">Широкие</button><button type="button" :class="{ active: skinModel === 'slim' }" @click="selectSkinModel('slim')">Стройные</button></div></div>
          <label class="workspace-check skin-elytra-toggle"><input v-model="skinElytraPreview" type="checkbox" :disabled="!selectedCape?.url" /><span><strong>Предпросмотр элитр</strong><small>{{ selectedCape?.url ? 'Показать текстуру плаща на крыльях элитр' : 'Выберите плащ, чтобы увидеть элитры' }}</small></span></label>
          <div v-if="selectedCape?.url" class="skin-cape-front"><CapeThumb :image="selectedCape.url" /><span><strong>{{ selectedCape.name || selectedCape.alias || 'Плащ' }}</strong><small>Лицевая сторона плаща</small></span></div>
        </div>
      </div>
      <div class="skin-library">
        <div class="skin-library-heading"><div><h3>Скины</h3><p>Текущий скин аккаунта и локальная библиотека.</p></div></div>
        <div class="skin-library-grid">
          <button v-if="activeRemoteSkin" class="skin-library-item skin-library-account-item" :class="{ active: !selectedSkinId }" type="button" @click="selectCurrentSkin">
            <span v-if="activeRemoteSkin.url" class="skin-library-model"><SkinThumb3D :image="activeRemoteSkin.url" :model="activeRemoteSkin.variant?.toUpperCase() === 'SLIM' ? 'slim' : 'classic'" :cape-image="activeCape?.url || ''" /></span>
            <span v-else class="skin-library-avatar-placeholder"><Icon name="skin" :size="18" /></span>
            <span>{{ skinProfileData?.name || 'Текущий скин' }}</span>
            <small>Профиль Minecraft · {{ activeRemoteSkin.variant?.toUpperCase() === 'SLIM' ? 'стройные руки' : 'широкие руки' }}</small>
            <span class="skin-library-current-badge">Экипирован</span>
          </button>
          <div v-for="skin in skinResults" :key="skin.skin_id" class="skin-library-item" :class="{ active: selectedSkinId === skin.skin_id }" @click="selectSkin(skin)">
            <span v-if="skin.image" class="skin-library-model"><SkinThumb3D :image="skin.image" :model="skin.model === 'slim' ? 'slim' : 'classic'" /></span>
            <span v-else class="skin-library-avatar-placeholder"><Icon name="skin" :size="18" /></span>
            <span>{{ skin.name }}</span>
            <small>{{ skin.model === 'slim' ? 'стройные' : 'широкие' }}</small>
            <button v-if="selectedSkinId === skin.skin_id" class="button button--small button--accent" type="button" @click.stop="applySelectedSkin(skin)">Установить</button>
          </div>
        </div>
        <div v-if="skinProfileData && !activeRemoteSkin && !skinResults.length" class="empty-mini"><Icon name="skin" :size="18" /><span>В профиле нет активного скина. Библиотека скинов пуста.</span></div>
        <div v-else-if="skinProfileData && !activeRemoteSkin" class="empty-mini"><Icon name="skin" :size="18" /><span>В профиле Minecraft нет активного скина.</span></div>
      </div>
      <div class="skin-cape-library">
        <div class="skin-library-heading"><div><h3>Плащи аккаунта</h3><p>Показываются плащи из профиля Minecraft. Если Mojang не вернул идентификатор плаща, его можно только просмотреть.</p></div><span class="skin-cape-count">{{ availableCapes.length }}</span></div>
        <div v-if="availableCapes.length" class="skin-cape-grid">
          <button v-for="cape in availableCapes" :key="cape.id || `${cape.alias || cape.name || 'cape'}-${cape.url || cape.state || ''}`" class="skin-cape-card" :class="{ active: (cape.id && selectedCapeId === cape.id) || (!cape.id && selectedCape === cape) }" type="button" :disabled="capeActionBusy || !cape.id" @click="selectCape(cape.id || '')">
            <span class="skin-cape-art"><CapeThumb v-if="cape.url" :image="cape.url" /><Icon v-else name="skin" :size="20" /></span>
            <span class="skin-cape-name">{{ cape.name || cape.alias || 'Плащ Minecraft' }}</span>
            <small>{{ !cape.id ? 'Только предпросмотр' : selectedCapeId === cape.id ? 'Выбран' : cape.state?.toUpperCase() === 'ACTIVE' ? 'Надет в Minecraft' : 'Доступен в профиле' }}</small>
          </button>
          <button class="skin-cape-card skin-cape-card--none" :class="{ active: !selectedCapeId }" type="button" :disabled="capeActionBusy" @click="selectCape('')">
            <span class="skin-cape-art"><Icon name="close" :size="20" /></span>
            <span class="skin-cape-name">Без плаща</span>
            <small>{{ selectedCapeId ? 'Снять текущий плащ' : 'Сейчас не надет' }}</small>
          </button>
        </div>
        <div v-else-if="skinProfileData" class="empty-mini"><Icon name="skin" :size="18" /><span>В профиле Minecraft пока нет доступных плащей.</span></div>
      </div>
      <div class="skin-panel-actions"><button class="button button--accent" type="button" @click="openSkinEditor"><Icon name="edit" :size="15" />Сделать скин</button><button class="button button--quiet" type="button" @click="openSkinsFolderBrowser"><Icon name="folder" :size="15" />Открыть папку со скинами</button><button class="button button--outline" type="button" @click="chooseSkinFile"><Icon name="upload" :size="15" />Импортировать файл скина</button><button class="button button--quiet" type="button" @click="resetSkin"><Icon name="refresh" :size="15" />Сбросить скин</button></div>
    </section>
    <Transition name="dialog">
      <SkinEditorDialog
        v-if="skinEditorOpen && skinAccountId"
        :account-id="skinAccountId"
        :account-name="skinEditorAccountName"
        :skins="skinResults"
        :current-image="skinPreviewImage"
        :current-model="currentSkinModel"
        @close="skinEditorOpen = false"
        @toast="(message, kind) => showToast(message, kind)"
        @saved="() => void onSkinEditorSaved()"
        @applied="() => void onSkinEditorApplied()"
      />
    </Transition>
  </section>
</template>
