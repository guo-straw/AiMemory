<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { History, X } from 'lucide-vue-next';
import { marked, type Tokens } from 'marked';
import type { MemoryRecord } from '../types';
import MarkdownView from './MarkdownView.vue';

const props = defineProps<{ memory: MemoryRecord }>();
const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLDialogElement>();
const markdown = computed(() => {
  const tokens = marked.lexer(props.memory.markdown);
  const start = tokens.findIndex(token => token.type === 'heading' && token.depth <= 2 && token.text === '更新日志');
  if (start < 0) return '';
  const heading = tokens[start] as Tokens.Heading;
  const end = tokens.findIndex((token, index) => index > start && token.type === 'heading' && token.depth <= heading.depth);
  return tokens.slice(start + 1, end < 0 ? undefined : end).map(token => token.raw).join('').trim();
});

onMounted(() => dialog.value?.showModal());
</script>

<template>
  <dialog ref="dialog" class="memory-log-dialog glass-raised" aria-labelledby="memory-log-title" data-testid="memory-update-log" @close="emit('close')" @click.self="dialog?.close()">
    <header class="memory-log-heading">
      <h2 id="memory-log-title"><History :size="19" />更新日志</h2>
      <button type="button" class="icon-button" aria-label="关闭更新日志" autofocus @click="dialog?.close()"><X :size="18" /></button>
    </header>
    <p class="memory-log-name">{{ memory.title }}</p>
    <div class="memory-log-content">
      <MarkdownView v-if="markdown" :markdown="markdown" />
      <p v-else class="memory-log-empty">这条记忆暂无更新日志。</p>
    </div>
  </dialog>
</template>

<style scoped>
.memory-log-dialog { width: 640px; max-width: calc(100vw - 48px); max-height: calc(100vh - 64px); margin: auto; padding: 24px; border-radius: var(--radius-shell); color: var(--fg); }
.memory-log-dialog::backdrop { background: var(--scrim); }
.memory-log-heading { display: flex; align-items: center; justify-content: space-between; gap: 20px; }
h2 { display: flex; align-items: center; gap: 10px; font-size: 21px; font-weight: 600; }
.memory-log-name { margin-top: 12px; color: var(--muted); font-size: var(--font-size-body); overflow-wrap: anywhere; }
.memory-log-content { margin-top: 24px; padding: 16px; border-radius: var(--radius-control); background: var(--bg); }
.memory-log-empty { color: var(--muted); font-size: var(--font-size-body); }
</style>
