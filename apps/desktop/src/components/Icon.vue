<script setup lang="ts">
withDefaults(defineProps<{ name: string; size?: number }>(), { size: 22 })
const paths: Record<string, string> = {
  copy: 'M9 5V3.5A1.5 1.5 0 0 1 10.5 2h7A1.5 1.5 0 0 1 19 3.5v11A1.5 1.5 0 0 1 17.5 16H16 M6.5 6h7A1.5 1.5 0 0 1 15 7.5v13a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 5 20.5v-13A1.5 1.5 0 0 1 6.5 6Z',
  history: 'M12 8v4l3 2 M4.5 5A9 9 0 1 1 3 16 M2 5h4V1',
  settings: 'm9 3 .7-1h4.6l.7 3 2 .9 2.7-.8 2.3 4-2 2.2v2.2l2 2.2-2.3 4-2.7-.8-2 .9-.7 2.9H9.7L9 19l-2-.9-2.7.8-2.3-4 2-2.2v-2.2L2 8.3l2.3-4L7 5.1 9 4 M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8',
  text: 'M6 2h12a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2 M8 7h8 M8 11h8 M8 15h8 M8 19h5',
  image: 'M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2 M3 17l5-5 4 4 4-6 5 7 M8 7h.01',
  files: 'M14 2H5a1 1 0 0 0-1 1v18a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1V8l-6-6Z M14 2v6h6',
  download: 'M12 2v13 m-5-5 5 5 5-5 M3 15v5a1 1 0 0 0 1 1h16a1 1 0 0 0 1-1v-5',
  eye: 'M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12 M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6',
  eyeOff: 'M3 3l18 18 M9.5 5.3A12 12 0 0 1 12 5c6.5 0 10 7 10 7a19 19 0 0 1-3 3.8 M6 6.2A22 22 0 0 0 2 12s3.5 7 10 7a12 12 0 0 0 5-1.1 M9.9 9.9a3 3 0 0 0 4.2 4.2',
  check: 'M21 11v1a9 9 0 1 1-5.3-8.2 M8 11l4 4L22 5',
  alert: 'M12 8v5 M12 17h.01 M10.3 3.9 1.8 18.4A1.7 1.7 0 0 0 3.3 21h17.4a1.7 1.7 0 0 0 1.5-2.6L13.7 3.9a2 2 0 0 0-3.4 0Z',
  close: 'm6 6 12 12 M6 18 18 6',
  minimize: 'M5 12h14',
  maximize: 'M5 5h14v14H5Z',
  restore: 'M9 8V4h11v11h-4 M4 9h11v11H4Z',
}
</script>

<template>
  <svg :width="size" :height="size" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path :d="paths[name] || paths.files" /></svg>
</template>
