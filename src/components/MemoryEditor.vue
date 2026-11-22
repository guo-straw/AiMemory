<script setup lang="ts">
import { computed, reactive, ref } from 'vue';
import { Check, FileText } from 'lucide-vue-next';
import type { MemoryInput, MemoryRecord, MemorySummary, Repository } from '../types';
import MarkdownView from './MarkdownView.vue';
import AppSelect from './AppSelect.vue';

const props = defineProps<{
  memory: MemoryRecord | null;
  repositories: Repository[];
  activeRepositoryId: string | null;
  allMemories: MemorySummary[];
  saving: boolean;
}>();
const emit = defineEmits<{ save: [input: MemoryInput]; cancel: [] }>();
const draft = reactive({
  repositoryId: props.memory?.repositoryId ?? props.activeRepositoryId ?? props.repositories[0]?.id ?? '',
  title: props.memory?.title ?? '',
  kind: props.memory?.kind ?? '技术决策',
  tags: props.memory?.tags.join(', ') ?? '',
  markdown: props.memory?.markdown ?? '## 结论\n\n\n## 详细说明\n\n\n## 原文依据\n\n',
  relatedIds: [...(props.memory?.outgoingRelatedIds ?? [])],
  source: props.memory?.source ?? '',
});
const initial = JSON.stringify(draft);
const dirty = computed(() => JSON.stringify(draft) !== initial);
const relationQuery = ref('');
const candidates = computed(() => props.allMemories.filter(memory => memory.id !== props.memory?.id &&
  `${memory.title} ${memory.id}`.toLowerCase().includes(relationQuery.value.toLowerCase())));
const canSave = computed(() => draft.repositoryId && draft.title.trim() && draft.markdown.trim());
const incomingIds = computed(() => props.memory?.relatedIds.filter(id => !props.memory?.outgoingRelatedIds.includes(id)) ?? []);
const repositoryOptions = computed(() => props.repositories.map(repo => ({ value: repo.id, label: repo.name })));
const kindOptions = computed(() => {
  const kinds = ['技术决策', '实现约束', '问题排查', '工作记录'];
  if (props.memory?.kind && !kinds.includes(props.memory.kind)) kinds.push(props.memory.kind);
  return kinds.map(kind => ({ value: kind, label: kind }));
});

function submit() {
  if (!canSave.value || props.saving) return;
  emit('save', {
    id: props.memory?.id,
    repositoryId: draft.repositoryId,
    title: draft.title.trim(),
    kind: draft.kind,
    tags: draft.tags.split(/[,，]/).map(tag => tag.trim()).filter(Boolean),
    markdown: draft.markdown,
    relatedIds: draft.relatedIds,
    source: draft.source.trim() || null,
    revision: props.memory?.revision,
  });
}

defineExpose({ dirty, submit });
</script>

<template>
  <form class="editor-page" @submit.prevent="submit" data-testid="memory-editor">
    <div class="page-heading">
      <div>
        <h1>{{ memory ? '编辑记忆' : '新增记忆' }}</h1>
        <p v-if="memory">{{ memory.id }}</p>
      </div>
      <span v-if="dirty" class="draft-state">有未保存的更改</span>
    </div>
    <div class="editor-meta">
      <label class="field">
        所属仓库
        <AppSelect v-model="draft.repositoryId" :options="repositoryOptions" :disabled="!!memory" aria-label="所属仓库" data-testid="editor-repository" />
      </label>
      <label class="field">
        记忆类型
        <AppSelect v-model="draft.kind" :options="kindOptions" aria-label="记忆类型" data-testid="editor-kind" />
      </label>
      <label class="field editor-title">
        记忆标题
        <input v-model="draft.title" required placeholder="例如，任务进度改用短轮询" data-testid="editor-title" />
      </label>
      <label class="field">
        标签
        <input v-model="draft.tags" placeholder="多个标签用逗号分隔" />
      </label>
    </div>
    <div class="editor-columns">
      <section class="edit-pane">
        <h2>
          <FileText :size="17" />Markdown 正文</h2>
        <textarea v-model="draft.markdown" aria-label="Markdown 正文" spellcheck="false" data-testid="editor-markdown" />
      </section>
      <section class="preview-pane">
        <h2>阅读预览</h2>
        <div class="preview-body">
          <h1>{{ draft.title || '记忆标题' }}</h1>
          <MarkdownView :markdown="draft.markdown" :title="draft.title" />
        </div>
      </section>
    </div>
    <div class="editor-extra">
      <label class="field">
        来源说明
        <input v-model="draft.source" placeholder="例如，Codex · 导出任务状态约定 · 轮次 12" />
      </label>
      <details class="relations-edit">
        <summary>关联记忆 <span>{{ draft.relatedIds.length + incomingIds.length }}</span>
        </summary>
        <input v-model="relationQuery" aria-label="筛选关联记忆" placeholder="搜索记忆标题或 ID" />
        <p v-if="incomingIds.length" class="relation-hint">由对方关联的条目，可在对方记忆中编辑。</p>
        <div class="relation-options">
          <label v-for="item in candidates" :key="item.id">
            <input v-if="incomingIds.includes(item.id)" type="checkbox" checked disabled />
            <input v-else v-model="draft.relatedIds" type="checkbox" :value="item.id" />
            <span>
              {{ item.title }}
              <small>{{ repositories.find(repo => repo.id === item.repositoryId)?.name }} · {{ item.id }}{{ incomingIds.includes(item.id) ? ' · 由对方关联' : '' }}</small>
            </span>
          </label>
          <p v-if="!candidates.length" class="muted">没有可选的记忆</p>
        </div>
      </details>
    </div>
    <footer class="editor-footer">
      <span>保存到本地 Markdown 文件 <kbd>⌘ / Ctrl S</kbd>
      </span>
      <div>
        <button type="button" class="button" :disabled="saving" @click="emit('cancel')">取消</button>
        <button class="button primary" :disabled="!canSave || saving" data-testid="save-memory">
          <Check :size="16" />{{ saving ? '正在保存…' : '保存记忆' }}
        </button>
      </div>
    </footer>
  </form>
</template>

<style scoped>
.editor-meta .app-select { width: 100%; min-height: 37px; height: 37px; padding: 7px 11px; }
</style>
