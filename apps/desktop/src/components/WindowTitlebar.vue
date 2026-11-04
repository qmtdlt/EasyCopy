<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { native } from '../bridge'
import Icon from './Icon.vue'

const emit = defineEmits<{ error: [message: string]; expanded: [value: boolean] }>()
const appWindow = native ? getCurrentWindow() : undefined
const maximized = ref(false)
const pending = ref(false)
const maximizeLabel = computed(() => maximized.value ? '还原' : '最大化')
let stopResize: (() => void) | undefined
let destroyed = false
let stateRevision = 0

function reportError(error: unknown) {
  if (!destroyed) emit('error', `窗口操作失败：${error instanceof Error ? error.message : String(error)}`)
}

async function refreshMaximized() {
  if (!appWindow) return
  const revision = ++stateRevision
  try {
    const [value, fullscreen] = await Promise.all([appWindow.isMaximized(), appWindow.isFullscreen()])
    if (!destroyed && revision === stateRevision) {
      maximized.value = value
      emit('expanded', value || fullscreen)
    }
  } catch (error) { reportError(error) }
}

async function act(action: 'minimize' | 'toggleMaximize' | 'close') {
  if (!appWindow || pending.value) return
  pending.value = true
  try {
    // close() follows the existing CloseRequested handler: hide and keep syncing.
    await appWindow[action]()
    if (action === 'toggleMaximize') await refreshMaximized()
  } catch (error) { reportError(error) }
  finally { pending.value = false }
}

async function drag(event: MouseEvent) {
  if (!appWindow) return
  // Use our own toggle: a borderless macOS window has no native zoom button
  // for Tauri's built-in drag-region maximizable check to inspect.
  if (event.detail === 2) { await act('toggleMaximize'); return }
  if (event.detail !== 1) return
  try { await appWindow.startDragging() }
  catch (error) { reportError(error) }
}

onMounted(async () => {
  if (!appWindow) return
  try {
    // Also track changes made by dragging, double-clicking or system shortcuts.
    const unlisten = await appWindow.onResized(() => { void refreshMaximized() })
    if (destroyed) { unlisten(); return }
    stopResize = unlisten
    await refreshMaximized()
  } catch (error) { reportError(error) }
})

onBeforeUnmount(() => { destroyed = true; stopResize?.() })
</script>

<template>
  <div class="titlebar">
    <div class="titlebar-drag" @mousedown.left.prevent="drag"></div>
    <div class="window-controls" role="group" aria-label="窗口控制">
      <button type="button" class="window-control" aria-label="最小化" title="最小化" :disabled="!native || pending" @click="act('minimize')"><Icon name="minimize" :size="16" /></button>
      <button type="button" class="window-control" :aria-label="maximizeLabel" :title="maximizeLabel" :disabled="!native || pending" @click="act('toggleMaximize')"><Icon :name="maximized ? 'restore' : 'maximize'" :size="16" /></button>
      <button type="button" class="window-control window-close" aria-label="关闭" title="关闭" :disabled="!native || pending" @click="act('close')"><Icon name="close" :size="18" /></button>
    </div>
  </div>
</template>
