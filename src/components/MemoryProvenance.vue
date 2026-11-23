<script setup lang="ts">
import type { MemoryRecord } from '../types';
import MarkdownView from './MarkdownView.vue';

defineProps<{ memory: MemoryRecord; adopting: boolean }>();
const emit = defineEmits<{ adopt: [sourceId: string] }>();
</script>

<template>
  <details v-if="memory.codePaths?.length" class="source-directory">
    <summary>涉及代码 · {{ memory.codePaths.length }} 个文件</summary>
    <ul class="code-paths"><li v-for="path in memory.codePaths" :key="path"><code>{{ path }}</code></li></ul>
  </details>
  <section v-if="memory.suggestions.length" class="conclusion-review" aria-label="结论与建议">
    <h2>当前结论 <small>{{ memory.conclusionOwner === 'manual' ? '人工确定' : '自动归档' }}</small></h2>
    <MarkdownView :markdown="memory.currentConclusion" />
    <article v-for="suggestion in memory.suggestions" :key="suggestion.sourceId" class="suggestion-review">
      <h3>待采用建议</h3>
      <MarkdownView :markdown="suggestion.summary" />
      <blockquote v-if="memory.evidenceSources.find(source => source.sourceId === suggestion.sourceId)?.quote">{{ memory.evidenceSources.find(source => source.sourceId === suggestion.sourceId)?.quote }}</blockquote>
      <small>{{ suggestion.sourceId }}</small>
      <button type="button" class="button" :disabled="adopting" @click="emit('adopt', suggestion.sourceId)">{{ adopting ? '正在采用…' : '采用此建议' }}</button>
    </article>
    <p class="muted">保留当前结论时可继续阅读。采用建议后记为人工确认。</p>
  </section>
  <details v-if="memory.evidenceSources.length" class="source-directory">
    <summary>按来源查看依据 · {{ memory.evidenceSources.length }}</summary>
    <details v-for="source in memory.evidenceSources" :key="source.sourceId" class="source-entry">
      <summary>会话 {{ source.sessionId }} · 轮次 {{ source.turnId }}{{ source.messageRole ? ` · ${source.messageRole === 'user' ? '用户' : '助手'}` : '' }}</summary>
      <small>{{ source.sourceId }}<template v-if="source.startLine"> · 正文第 {{ source.startLine }} 行</template></small>
      <blockquote v-if="source.status === 'located'">{{ source.quote }}</blockquote>
      <p v-else>这条来源无法在当前文件中精确定位，可查看正文中的整节原文依据。</p>
    </details>
  </details>
</template>

<style scoped>
.conclusion-review, .source-directory { margin: 22px 0; padding: 18px; border: 1px solid var(--line); border-radius: 12px; }
h2, h3 { margin: 0 0 12px; font-size: 15px; }
h2 small { margin-left: 8px; font-size: 12px; color: var(--muted); font-weight: 400; }
.suggestion-review { margin-top: 18px; padding-top: 18px; border-top: 1px solid var(--line); }
small { display: block; overflow-wrap: anywhere; color: var(--muted); }
h2 small { display: inline; }
blockquote { margin: 12px 0; padding-left: 14px; border-left: 2px solid var(--accent); white-space: pre-wrap; overflow-wrap: anywhere; }
.button { margin-top: 12px; }
summary { cursor: pointer; overflow-wrap: anywhere; }
.source-entry { margin-top: 14px; }
.source-entry small { margin-top: 8px; }
.code-paths { padding-left: 18px; margin: 12px 0 0; line-height: 1.9; overflow-wrap: anywhere; }
</style>
