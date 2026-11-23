<script setup lang="ts">
import type { ArchiveStatus, MemorySummary } from '../types';
import ArchivePanel from './ArchivePanel.vue';
import UsagePanel from './UsagePanel.vue';
defineProps<{ modelValue: 'usage' | 'archive'; status: ArchiveStatus | null; memories: MemorySummary[]; readError: string; retrying: boolean }>();
const emit = defineEmits<{ 'update:modelValue': [value: 'usage' | 'archive']; retry: []; settings: []; openMemory: [id: string] }>();
</script>

<template>
  <section class="archive-usage-page">
    <h1>归档与用量</h1>
    <nav class="usage-tabs usage-page-tabs" aria-label="归档与用量视图">
      <button :class="{ active: modelValue === 'usage' }" :aria-current="modelValue === 'usage' ? 'page' : undefined" data-testid="codex-usage-tab" @click="emit('update:modelValue', 'usage')">Codex 用量</button>
      <button :class="{ active: modelValue === 'archive' }" :aria-current="modelValue === 'archive' ? 'page' : undefined" data-testid="memory-archive-tab" @click="emit('update:modelValue', 'archive')">记忆归档</button>
    </nav>
    <UsagePanel v-if="modelValue === 'usage'" @settings="emit('settings')" @open-memory="emit('openMemory', $event)" />
    <ArchivePanel v-else embedded :status="status" :memories="memories" :read-error="readError" :retrying="retrying" @retry="emit('retry')" @settings="emit('settings')" @open-memory="emit('openMemory', $event)" />
  </section>
</template>

<style scoped>
.archive-usage-page { position: relative; flex: 1; padding: 30px 36px 36px; overflow-y: auto; min-height: 0; }
h1 { margin: 0 0 16px; font-size: 24px; font-weight: 650; letter-spacing: -.4px; }
.usage-page-tabs { border-bottom: 1px solid var(--line); margin-bottom: 20px; }
.archive-usage-page :deep(.archive-page) { padding: 0; overflow: visible; }
.archive-usage-page :deep(.archive-page > .page-heading) { justify-content: flex-end; margin-bottom: 16px; }
@media (max-width: 1100px) { .archive-usage-page { padding: 28px 26px; } }
@media (max-width: 700px) { .archive-usage-page { padding: 24px 18px; } }
</style>
