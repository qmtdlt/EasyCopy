<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { api, native } from '../bridge'
import type { AutostartStatus } from '../types'

const status = ref<AutostartStatus | null>(null)
const busy = ref(false)
const error = ref('')
const message = ref('')
let disposed = false

function errorText(value: unknown) { return value instanceof Error ? value.message : String(value) }
async function refresh() {
  if (busy.value || disposed) return
  busy.value = true
  try {
    status.value = await api.autostart()
    error.value = ''
    message.value = ''
  } catch (cause) {
    status.value = null
    error.value = errorText(cause)
  } finally { busy.value = false }
}
async function toggle() {
  if (!status.value || busy.value) return
  const enabled = !status.value.enabled
  busy.value = true; error.value = ''; message.value = ''
  try {
    status.value = await api.setAutostart(enabled)
    message.value = enabled ? '已开启，下次登录系统时在托盘后台启动。' : '已关闭开机自启动。'
  } catch (cause) {
    error.value = errorText(cause)
    // An OS write can partially succeed. Re-read instead of assuming rollback.
    try { status.value = await api.autostart() }
    catch (readError) { status.value = null; error.value += `；${errorText(readError)}` }
  } finally { busy.value = false }
}
onMounted(() => { void refresh(); window.addEventListener('focus', refresh) })
onBeforeUnmount(() => { disposed = true; window.removeEventListener('focus', refresh) })
</script>

<template>
  <div class="autostart-setting">
    <div class="autostart-row">
      <div><span id="autostart-label">开机自启动</span><p id="autostart-hint">登录系统后在托盘后台运行，不弹出主窗口。更改立即保存。</p></div>
      <button type="button" class="autostart-switch" role="switch" aria-labelledby="autostart-label" aria-describedby="autostart-hint autostart-result"
        :aria-checked="status?.enabled ?? false" :aria-busy="busy"
        :disabled="busy || !status || (!status.canEnable && !status.enabled)" @click="toggle"><span></span></button>
    </div>
    <p id="autostart-result" class="autostart-result" :class="{ 'error-text': !!error }" role="status">
      {{ busy ? '正在读取或更新系统设置…' : error || status?.reason || message }}
      <button v-if="error && native && !busy" type="button" class="autostart-retry" @click="refresh">重试读取</button>
    </p>
  </div>
</template>
