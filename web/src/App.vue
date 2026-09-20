<script setup lang="ts">
import { computed } from 'vue'
import { useLocaleStore } from '@/stores/locale'
import { ConfigProvider as TConfigProvider } from 'tdesign-vue-next'
import enUS from 'tdesign-vue-next/es/locale/en_US'
import zhCN from 'tdesign-vue-next/es/locale/zh_CN'
import AppShell from '@/components/layout/AppShell.vue'

const localeStore = useLocaleStore()

// tdesign's own strings (pagination, empty states, date pickers) follow the app language.
const tdesignLocale = computed(() => (localeStore.locale === 'zh-CN' ? zhCN : enUS))
</script>

<template>
  <TConfigProvider :locale="tdesignLocale">
    <AppShell />
  </TConfigProvider>
</template>
