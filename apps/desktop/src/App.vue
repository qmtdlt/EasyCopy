<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { api, events, native } from './bridge'
import type { ConnectionState, ContentKind, HistoryEntry, SettingsInput } from './types'
import Icon from './components/Icon.vue'
import WindowTitlebar from './components/WindowTitlebar.vue'
import AutostartSetting from './components/AutostartSetting.vue'
import FileUploadConfirmation from './components/FileUploadConfirmation.vue'
import FileUploadSetting from './components/FileUploadSetting.vue'

const previewUrl = ref('')
let previewRequest = 0
function closePreview() { previewRequest++; if (previewUrl.value) URL.revokeObjectURL(previewUrl.value); previewUrl.value = '' }
async function openPreview(item: HistoryEntry) {
  if (!item.available) return
  closePreview()
  const request = previewRequest
  try {
    const data = await api.previewImage(item.id)
    if (request === previewRequest) previewUrl.value = URL.createObjectURL(new Blob([data], { type: 'image/png' }))
  } catch (error) { if (request === previewRequest) showNotice(errorText(error), true) }
}
function previewKey(event: KeyboardEvent) { if (event.key === 'Escape') closePreview() }
const windowExpanded = ref(false)
const page = ref<'history' | 'settings'>('history')
const filter = ref<'all' | ContentKind>('all')
const history = ref<HistoryEntry[]>([])
const connection = ref<ConnectionState>({ connected: false, message: native ? '正在连接…' : '浏览器预览 · 未连接' })
const form = ref<SettingsInput>({ address: '', port: 8443, pairingKey: '' })
const hasPairingKey = ref(false)
const revealKey = ref(false)
const busy = ref<'test' | 'save' | null>(null)
const rowBusy = ref<string | null>(null)
const formMessage = ref('')
const formError = ref(false)
const notice = ref('')
const noticeError = ref(false)
const loading = ref(true)
const items = computed(() => history.value.filter(item => filter.value === 'all' || item.kind === filter.value))
const filters: { value: 'all' | ContentKind; label: string }[] = [
  { value: 'all', label: '全部' }, { value: 'text', label: '文本' }, { value: 'image', label: '图片' }, { value: 'files', label: '文件' },
]
let stopEvents: (() => void) | undefined
let noticeTimer: ReturnType<typeof setTimeout> | undefined
let destroyed = false
let historyRevision = 0
let connectionRevision = 0

function errorText(error: unknown) { return error instanceof Error ? error.message : String(error) }
function showNotice(message: string, error = false) {
  notice.value = message; noticeError.value = error
  if (noticeTimer) clearTimeout(noticeTimer)
  noticeTimer = setTimeout(() => { notice.value = '' }, error ? 8000 : 3000)
}
function clock(timestamp: number) { return new Date(timestamp).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }) }
function exactTime(timestamp: number) { return new Date(timestamp).toLocaleString('zh-CN', { hour12: false }) }
function available(item: HistoryEntry) { return item.available }
function statusText(item: HistoryEntry) {
  if (item.error) return item.error
  const labels: Record<string, string> = { preparing: '正在准备…', pending: '等待同步…', registered: '等待传输…', uploading: '正在上传…', downloading: '正在下载…', verifying: '正在校验…', failed: '同步失败', cancelled: '已取消' }
  return labels[item.status] || ''
}
function changed() { formMessage.value = ''; formError.value = false }

async function submit(action: 'test' | 'save') {
  formMessage.value = ''; formError.value = false
  if (!form.value.address.trim()) { formMessage.value = '请输入服务器地址'; formError.value = true; return }
  if (!Number.isInteger(Number(form.value.port)) || Number(form.value.port) < 1 || Number(form.value.port) > 65535) { formMessage.value = '端口应为 1–65535'; formError.value = true; return }
  if (!form.value.pairingKey.trim() && !hasPairingKey.value) { formMessage.value = '请粘贴服务器提供的连接密钥'; formError.value = true; return }
  busy.value = action
  const input = { ...form.value, address: form.value.address.trim(), port: Number(form.value.port), pairingKey: form.value.pairingKey.trim() }
  try {
    if (action === 'test') {
      await api.test(input)
      formMessage.value = '连接正常'
    } else {
      await api.save(input)
      hasPairingKey.value = true
      form.value.pairingKey = ''
      revealKey.value = false
      formMessage.value = '设置已保存'
    }
  } catch (error) { formMessage.value = errorText(error); formError.value = true }
  finally { busy.value = null }
}

async function act(item: HistoryEntry, action: 'copy' | 'save') {
  rowBusy.value = item.id
  try {
    if (action === 'copy') { await api.copy(item.id); showNotice('已复制') }
    else await api.saveItem(item.id)
  } catch (error) { showNotice(errorText(error), true) }
  finally { rowBusy.value = null }
}

onMounted(async () => {
  window.addEventListener('keydown', previewKey)
  try {
    stopEvents = await events({
      history: entries => { historyRevision++; history.value = entries },
      connection: state => {
        const revision = ++connectionRevision
        connection.value = state
        // Credential loading can finish after the first settings request. Refresh
        // only its availability; never replace fields the user is editing.
        void api.settings().then(settings => {
          if (!destroyed && connectionRevision === revision) hasPairingKey.value = settings.hasPairingKey
        }).catch(() => {})
      },
      transfer: event => { if (event.phase === 'failed') showNotice(event.error || '传输失败', true) },
    })
    if (destroyed) { stopEvents(); return }
    const historyAtRequest = historyRevision, connectionAtRequest = connectionRevision
    const [entries, settings, state] = await Promise.all([api.history(), api.settings(), api.connection()])
    if (historyRevision === historyAtRequest) history.value = entries
    if (connectionRevision === connectionAtRequest) connection.value = state
    form.value.address = settings.address; form.value.port = settings.port
    if (connectionRevision === connectionAtRequest) hasPairingKey.value = settings.hasPairingKey
  } catch (error) { showNotice(errorText(error), true) }
  finally { loading.value = false }
})
onBeforeUnmount(() => { destroyed = true; window.removeEventListener('keydown', previewKey); closePreview(); stopEvents?.(); if (noticeTimer) clearTimeout(noticeTimer) })
</script>

<template>
  <div class="app-shell" :class="{ 'window-expanded': windowExpanded }">
    <WindowTitlebar @expanded="windowExpanded = $event" @error="showNotice($event, true)" />
    <aside class="sidebar">
      <div class="brand" data-tauri-drag-region><Icon name="copy" :size="29" /><span>EasyCopy</span></div>
      <nav aria-label="主菜单">
        <button :class="{ selected: page === 'history' }" :aria-current="page === 'history' ? 'page' : undefined" @click="page = 'history'"><Icon name="history" /><span>剪贴板历史</span></button>
        <button :class="{ selected: page === 'settings' }" :aria-current="page === 'settings' ? 'page' : undefined" @click="page = 'settings'"><Icon name="settings" /><span>设置</span></button>
      </nav>
      <div class="connection" :class="{ online: connection.connected }" role="status">
        <div><span class="connection-dot"></span><span>{{ connection.connected ? '已连接' : native ? '未连接' : '浏览器预览' }}</span></div>
        <p :title="connection.message">{{ connection.connected ? 'Linux 中转服务器' : connection.message }}</p>
      </div>
    </aside>
    <main class="content">
      <h1>{{ page === 'history' ? '剪贴板历史' : '设置' }}</h1>
      <template v-if="page === 'history'">
        <div class="filters" role="group" aria-label="历史类型">
          <button v-for="item in filters" :key="item.value" :aria-pressed="filter === item.value" @click="filter = item.value">{{ item.label }}</button>
        </div>
        <div v-if="loading" class="empty-state" role="status">正在读取历史…</div>
        <div v-else-if="!items.length" class="empty-state">
          <Icon :name="filter === 'all' ? 'copy' : filter" :size="34" />
          <p>{{ filter === 'all' ? '还没有剪贴板历史' : `暂无${filters.find(item => item.value === filter)?.label}记录` }}</p>
          <span>{{ native ? '复制文本、图片或文件后，会显示在这里。' : '请启动桌面客户端，查看真实剪贴板记录。' }}</span>
        </div>
        <ul v-else class="history-list" aria-label="剪贴板记录">
          <li v-for="item in items" :key="item.id" class="history-row" tabindex="0" :class="{ 'row-pending': !available(item) }">
            <div class="item-icon"><Icon :name="item.kind" :size="25" /></div>
            <div class="item-origin"><time :datetime="new Date(item.createdAt).toISOString()" :title="exactTime(item.createdAt)">{{ clock(item.createdAt) }}</time><span :title="item.deviceName">{{ item.deviceName }}</span></div>
            <div class="item-body">
              <button v-if="item.kind === 'image' && item.thumbnail" class="thumbnail-button" :disabled="!available(item)" aria-label="放大预览图片" @click="openPreview(item)"><img class="thumbnail" :src="item.thumbnail" alt="剪贴板图片缩略图" /></button>
              <div class="item-description"><p :title="item.preview">{{ item.preview || (item.kind === 'image' ? '图片' : item.kind === 'files' ? '文件' : '空文本') }}</p><span :class="{ 'error-text': !!item.error }" :title="item.error || item.detail">{{ statusText(item) || item.detail || (item.kind === 'text' ? '文本' : '') }}</span></div>
            </div>
            <div class="item-actions">
              <button v-if="item.kind !== 'files'" :disabled="!available(item) || rowBusy === item.id" @click="act(item, 'copy')"><Icon name="copy" :size="17" />复制</button>
              <button :disabled="!available(item) || rowBusy === item.id" @click="act(item, 'save')"><Icon name="download" :size="17" />另存为</button>
            </div>
          </li>
        </ul>
      </template>
      <section v-else class="settings" aria-labelledby="server-heading">
        <h2 id="server-heading">中转服务器</h2>
        <form @submit.prevent="submit('save')">
          <fieldset :disabled="busy !== null">
            <div class="server-fields">
              <label class="address-field">服务器地址<input v-model="form.address" name="address" placeholder="192.168.1.10" spellcheck="false" autocomplete="off" @input="changed" /></label>
              <label class="port-field">端口<input v-model.number="form.port" name="port" type="number" min="1" max="65535" step="1" inputmode="numeric" @input="changed" /></label>
            </div>
            <label class="key-label" for="pairing-key">连接密钥</label>
            <div class="secret-field">
              <input id="pairing-key" v-model="form.pairingKey" :type="revealKey ? 'text' : 'password'" :placeholder="hasPairingKey ? '已保存 · 留空保留现有密钥' : '粘贴服务器提供的连接密钥'" spellcheck="false" autocomplete="off" autocapitalize="off" @input="changed" />
              <button type="button" class="reveal-key" :aria-label="revealKey ? '隐藏连接密钥' : '显示连接密钥'" :aria-pressed="revealKey" @click="revealKey = !revealKey"><Icon :name="revealKey ? 'eyeOff' : 'eye'" :size="21" /></button>
            </div>
            <div class="form-footer">
              <div class="form-result" :class="{ error: formError }" role="status"><template v-if="formMessage"><Icon :name="formError ? 'alert' : 'check'" :size="22" /><span>{{ formMessage }}</span></template></div>
              <button class="button secondary" type="button" @click="submit('test')">{{ busy === 'test' ? '测试中…' : '测试连接' }}</button>
              <button class="button primary" type="submit">{{ busy === 'save' ? '保存中…' : '保存' }}</button>
            </div>
          </fieldset>
        </form>
        <AutostartSetting />
        <FileUploadSetting />
      </section>
    </main>
    <div v-if="notice" class="toast" :class="{ 'toast-error': noticeError }" role="status"><Icon :name="noticeError ? 'alert' : 'check'" :size="18" /><span>{{ notice }}</span><button aria-label="关闭提示" @click="notice = ''"><Icon name="close" :size="16" /></button></div>
    <div v-if="previewUrl" class="image-preview" role="dialog" aria-modal="true" aria-label="图片预览" @click.self="closePreview">
      <button class="preview-close" aria-label="关闭图片预览" @click="closePreview"><Icon name="close" :size="24" /></button>
      <img :src="previewUrl" alt="剪贴板图片预览" />
    </div>
    <FileUploadConfirmation />
  </div>
</template>
