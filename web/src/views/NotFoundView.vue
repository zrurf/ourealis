<script setup lang="ts">
// Fallback for an unknown path; the router matches it last.
import { useI18n } from 'vue-i18n'
import PagePlaceholder from '@/components/common/PagePlaceholder.vue'

const { t } = useI18n({ useScope: 'global' })
</script>

<template>
  <PagePlaceholder :title="t('views.notFound.title')" />
</template>
