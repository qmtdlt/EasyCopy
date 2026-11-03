<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { api, events } from '../bridge'
import { TransferIndicator } from '../transfer-indicator.mjs'
import { loadTransferArt } from '../transfer-art'
import Icon from './Icon.vue'

const canvas = ref<HTMLCanvasElement>()
const opacity = ref(0)
const failed = ref(false)
const label = ref('')
let indicator: TransferIndicator | undefined
let stopEvents: (() => void) | undefined
let destroyed = false
const motionPreference = matchMedia('(prefers-reduced-motion: reduce)')
const updateMotion = () => indicator?.setReducedMotion(motionPreference.matches)

onMounted(async () => {
  try {
    const drawArt = await loadTransferArt()
    if (destroyed) return
    indicator = new TransferIndicator({
      fadeMs: motionPreference.matches ? 0 : 140,
      draw: state => {
        if (canvas.value) drawArt(canvas.value, state.direction, state.progress)
        opacity.value = state.opacity
        failed.value = state.phase === 'failed'
        label.value = failed.value ? '传输失败' : state.phase === 'complete' ? '传输完成' : state.direction === 'upload' ? '正在上传' : '正在下载'
      },
      onHidden: taskId => { void api.overlayHidden(taskId).catch(error => console.error('悬浮窗口隐藏失败', error)) },
    })
    motionPreference.addEventListener('change', updateMotion)
    stopEvents = await events({ transfer: event => { indicator?.accept(event) } })
    if (destroyed) { stopEvents(); indicator.destroy(); return }
    await api.overlayReady()
  } catch (error) {
    console.error('悬浮图标初始化失败', error)
  }
})
onBeforeUnmount(() => { destroyed = true; stopEvents?.(); indicator?.destroy(); motionPreference.removeEventListener('change', updateMotion) })
</script>

<template>
  <div class="transfer-overlay" role="status" :aria-label="label">
    <div class="transfer-art" :class="{ failed }" :style="{ opacity }">
      <canvas ref="canvas" aria-hidden="true"></canvas>
      <span v-if="failed" class="failure-mark"><Icon name="close" :size="12" /></span>
    </div>
  </div>
</template>
