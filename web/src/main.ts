import { createPinia } from 'pinia'
import { createApp } from 'vue'
import 'tdesign-vue-next/es/style/index.css'
import '@/styles/index.css'
import App from '@/App.vue'
import { i18n } from '@/locales'
import { router } from '@/router'
import { useThemeStore } from '@/stores/theme'

const app = createApp(App)

app.use(createPinia())
app.use(i18n)
app.use(router)

// Applying the stored theme before mounting keeps a dark session from flashing light.
useThemeStore()

app.mount('#app')
