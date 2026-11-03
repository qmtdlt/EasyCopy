<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { api, native } from '../bridge'

const mb = ref<number | string>(50)
const loading = ref(true)
const loaded = ref(false)
const busy = ref(false)
const message = ref('')
const error = ref(false)
async function load() {
  loading.value = true
  try { mb.value = await api.fileUploadThreshold(); loaded.value = true; message.value = ''; error.value = false }
  catch (e) { message.value = String(e); error.value = true }
  finally { loading.value = false }
}
async function save() {
  const value = Number(mb.value)
  if (!Number.isSafeInteger(value) || value < 1 || value > Math.floor(Number.MAX_SAFE_INTEGER / 1_000_000)) {
    message.value = '请输入有效的正整数，单位为 MB'; error.value = true; return
  }
  busy.value = true
  message.value = ''
  try { await api.setFileUploadThreshold(value); message.value = '已保存，从下一次复制文件生效'; error.value = false }
  catch (e) { message.value = String(e); error.value = true }
  finally { busy.value = false }
}
onMounted(load)
</script>

<template>
  <section class="file-upload-setting" aria-labelledby="file-upload-setting-title">
    <h3 id="file-upload-setting-title">文件上传确认</h3>
    <p id="file-upload-setting-help">本次复制的文件总大小超过此值时，先询问是否上传，默认选择“否”。</p>
    <form @submit.prevent="save">
      <label for="file-upload-threshold">确认阈值</label>
      <input id="file-upload-threshold" v-model="mb" name="file-upload-threshold" type="number" min="1" max="9007199254" step="1" required aria-describedby="file-upload-setting-help" :disabled="loading || busy || !loaded || !native" @input="message = ''" />
      <span>MB</span>
      <button class="button secondary" :disabled="loading || busy || !loaded || !native">{{ busy ? '保存中…' : '保存阈值' }}</button>
    </form>
    <p v-if="message" class="file-upload-setting-result" :class="{ 'error-text': error }" role="status">{{ message }} <button v-if="!loaded" type="button" @click="load">重试</button></p>
  </section>
</template>

<style scoped>
.file-upload-setting { max-width: 550px; margin-top: 24px; padding: 20px 0; border-top: 1px solid var(--line); }
h3 { margin: 0; font-size: 16px; font-weight: 500; }
p { margin: 8px 0 14px; color: var(--muted); font-size: 12px; line-height: 1.6; }
form { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
label, span { font-size: 14px; }
input { width: 120px; }
.file-upload-setting-result { margin-bottom: 0; }
.file-upload-setting-result button { text-decoration: underline; }
</style>
