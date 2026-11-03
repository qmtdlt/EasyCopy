import { createApp } from 'vue'
import App from './App.vue'
import Overlay from './components/Overlay.vue'
import './style.css'

const isOverlay = new URLSearchParams(location.search).has('overlay')
document.documentElement.classList.toggle('overlay-document', isOverlay)
createApp(isOverlay ? Overlay : App).mount('#app')
