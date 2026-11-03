<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { api, events, type FileUploadPrompt } from '../bridge'

const dialog = ref<HTMLDialogElement>()
const noButton = ref<HTMLButtonElement>()
const prompt = ref<FileUploadPrompt | null>(null)
const busy = ref(false)
const error = ref('')
let revision = 0
let destroyed = false
let stopEvents: (() => void) | undefined

async function display(value: FileUploadPrompt | null) {
  prompt.value = value
  busy.value = false
  error.value = ''
  await nextTick()
  if (destroyed) return
  if (prompt.value) {
    if (!dialog.value?.open) dialog.value?.showModal()
    noButton.value?.focus()
  } else dialog.value?.close()
}
async function answer(upload: boolean) {
  const id = prompt.value?.id
  if (!id || busy.value) return
  busy.value = true
  try {
    await api.resolveFileUpload(id, upload)
    if (prompt.value?.id === id) await display(null)
  } catch (e) {
    if (prompt.value?.id === id) {
      error.value = e instanceof Error ? e.message : String(e)
      busy.value = false
      await nextTick()
      noButton.value?.focus()
    }
  }
}
onMounted(async () => {
  try {
    stopEvents = await events({ fileUpload: value => { revision++; void display(value) } })
    if (destroyed) { stopEvents(); return }
    const requestedAt = revision
    const value = await api.fileUploadPrompt()
    if (!destroyed && revision === requestedAt) await display(value)
  } catch { /* No response never authorizes an upload. */ }
})
onBeforeUnmount(() => { destroyed = true; stopEvents?.() })
</script>

<template>
  <dialog ref="dialog" class="file-upload-dialog" aria-labelledby="file-upload-title" aria-describedby="file-upload-description" @cancel.prevent="answer(false)">
    <template v-if="prompt">
      <button class="file-upload-close" aria-label="关闭上传询问" :disabled="busy" @click="answer(false)">×</button>
      <h2 id="file-upload-title">是否上传到 EasyCopy？</h2>
      <p id="file-upload-description">本次复制的文件共 {{ (Math.ceil(prompt.bytes / 10_000) / 100).toFixed(2) }} MB，超过 {{ prompt.thresholdMb }} MB。</p>
      <p class="file-upload-hint">选择“否”仍可在本机粘贴，本次文件不会同步。</p>
      <p v-if="error" role="alert">{{ error }}</p>
      <div class="file-upload-actions">
        <button class="button secondary" :disabled="busy" @click="answer(true)">是，上传</button>
        <button ref="noButton" class="button primary" autofocus :disabled="busy" @click="answer(false)">否</button>
      </div>
    </template>
  </dialog>
</template>

<style scoped>
.file-upload-dialog { position: fixed; inset: 0; margin: auto; width: min(440px, calc(100vw - 48px)); padding: 30px; border: 1px solid #756b63; border-radius: 16px; color: #f5f1ed; background: #302c29; box-shadow: 0 20px 70px #0003; }
.file-upload-dialog::backdrop { background: #211c1855; }
h2 { margin: 0 0 18px; font-size: 20px; }
p { margin: 10px 0; line-height: 1.6; font-size: 14px; }
.file-upload-hint { color: #b4a79b; }
.file-upload-close { position: absolute; top: 8px; right: 12px; border: 0; background: transparent; font-size: 24px; cursor: pointer; }
.file-upload-actions { display: flex; justify-content: flex-end; gap: 12px; margin-top: 24px; }
button:focus-visible { outline: 3px solid #d3b58e; outline-offset: 3px; }
</style>
