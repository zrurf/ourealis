<script setup lang="ts">
import { useI18n } from 'vue-i18n'

defineProps<{
  /** Already-translated page title, rendered as the view's heading. */
  title: string
}>()

const { t } = useI18n({ useScope: 'global' })
</script>

<template>
  <section class="mx-auto max-w-5xl px-8 py-8">
    <h1 class="font-semibold text-ink">{{ title }}</h1>
    <p class="mt-2 text-sm text-muted">{{ t('common.empty') }}</p>
  </section>
</template>
