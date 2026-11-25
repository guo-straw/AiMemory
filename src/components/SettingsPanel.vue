<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue';
import { Archive, Check, Clock3, FolderOpen, GitBranch, Plus, RefreshCw } from 'lucide-vue-next';
import { chooseCodexDirectory, chooseHarnessDirectory } from '../api';
import { createServiceDraft, serviceInput } from '../model-services';
import type { AppSettings, ClientSource, Repository, SettingsInput, Workspace } from '../types';
import AppUpdate from './AppUpdate.vue';
import ModelService from './ModelService.vue';
import McpConnection from './McpConnection.vue';
import GeneralSettings from './GeneralSettings.vue';

const props = defineProps<{
  settings: AppSettings | null;
  workspace: Workspace;
  saving: boolean;
  loading: boolean;
  transferring?: boolean;
}>();
const emit = defineEmits<{
  save: [input: SettingsInput];
  chooseWorkspace: [];
  openWorkspace: [];
  refresh: [];
  addRepository: [];
  repositoryMenu: [event: MouseEvent, repository: Repository];
}>();
const tab = ref('models');
const tabs = [
  { key: 'general', label: '通用' },
  { key: 'models', label: '模型服务' }, { key: 'library', label: '记忆库与仓库' },
  { key: 'archive', label: '自动归档' }, { key: 'mcp', label: 'MCP 接入' },
  { key: 'theme', label: '主题与外观' }, { key: 'transfer', label: '导入与导出' },
  { key: 'update', label: '应用更新' },
];
const draft = reactive({
  chat: createServiceDraft('chat'),
  embedding: createServiceDraft('embedding'),
  codexDirectory: '', harnessDirectory: '', archiveSources: ['codex'] as ClientSource[],
  archiveEnabled: false, runInBackground: false,
});
function chooseTab(key: string) {
  tab.value = key;
}
const directoryError = ref('');
const initial = ref('');
const dirty = computed(() => initial.value !== '' && JSON.stringify(draft) !== initial.value);
const showSave = computed(() => ['models', 'archive'].includes(tab.value) || dirty.value);

function reset() {
  const settings = props.settings;
  if (!settings) return;
  draft.chat = createServiceDraft('chat', settings.chat);
  draft.embedding = createServiceDraft('embedding', settings.embedding);
  draft.codexDirectory = settings.codexDirectory;
  draft.harnessDirectory = settings.harnessDirectory ?? '';
  draft.archiveSources = [...(settings.archiveSources ?? ['codex'])];
  draft.archiveEnabled = settings.archiveEnabled;
  draft.runInBackground = settings.runInBackground;
  initial.value = JSON.stringify(draft);
}

watch(() => props.settings, reset, { immediate: true });
function submit() {
  if (props.saving || props.loading || props.transferring || !props.settings) return;
  emit('save', {
    chat: serviceInput(draft.chat, props.settings.chat),
    embedding: serviceInput(draft.embedding, props.settings.embedding),
    codexDirectory: draft.codexDirectory,
    harnessDirectory: draft.harnessDirectory,
    archiveSources: [...draft.archiveSources],
    archiveEnabled: draft.archiveEnabled,
    runInBackground: draft.runInBackground,
  });
}

async function pickCodexDirectory() {
  directoryError.value = '';
  try {
    const directory = await chooseCodexDirectory();
    if (directory) draft.codexDirectory = directory;
  } catch (error) {
    directoryError.value = String(error);
  }
}

async function pickHarnessDirectory() {
  directoryError.value = '';
  try {
    const directory = await chooseHarnessDirectory();
    if (directory) draft.harnessDirectory = directory;
  } catch (error) {
    directoryError.value = String(error);
  }
}

function dateTime(value: string) {
  return new Date(value).toLocaleString('zh-CN', { hour12: false });
}

defineExpose({ dirty, submit, reset });
</script>

<template>
  <section class="settings-page integration-settings" data-testid="api-settings">
    <div class="page-heading">
      <div>
        <h1>设置与接入</h1>
      </div>
      <div class="settings-save">
        <span v-if="dirty" class="draft-state">有未保存的更改</span>
        <button type="button" class="button primary" :class="{ 'settings-save-placeholder': !showSave }" :aria-hidden="!showSave" :disabled="!showSave || saving || loading || transferring || !settings" data-testid="save-settings" @click="submit">
          <Check :size="16" />{{ saving ? '正在保存…' : '保存设置' }}
        </button>
      </div>
    </div>

    <nav class="settings-tabs" aria-label="设置分类">
      <button v-for="item in tabs" :key="item.key" type="button" :class="{ active: tab === item.key }" :disabled="transferring" :aria-current="tab === item.key ? 'page' : undefined" :data-testid="`settings-tab-${item.key}`" @click="chooseTab(item.key)">{{ item.label }}</button>
    </nav>

    <GeneralSettings v-if="tab === 'general'" :disabled="saving || loading || !!transferring" />

    <div v-if="settings" v-show="tab === 'models'" class="settings-content-grid">
      <ModelService v-model="draft.chat" kind="chat" :saved="settings.chat" :disabled="saving || loading" />
      <ModelService v-model="draft.embedding" kind="embedding" :saved="settings.embedding" :disabled="saving || loading" />
    </div>
    <p v-else class="settings-empty">正在读取模型配置…</p>

    <section v-show="tab === 'library'" class="settings-content-grid" data-testid="library-settings">
      <div class="settings-main-panel">
        <header class="settings-section-title"><FolderOpen :size="18" /><h2>记忆库目录</h2></header>
        <p class="settings-path" data-testid="workspace-path">{{ workspace.root }}</p>
        <div class="settings-actions">
          <button type="button" class="button" :disabled="loading || saving" data-testid="migrate-workspace" @click="emit('chooseWorkspace')"><FolderOpen :size="15" />{{ loading ? '正在处理…' : '更改目录并迁移' }}</button>
          <button type="button" class="text-button" :disabled="loading || saving" data-testid="open-workspace" @click="emit('openWorkspace')">打开已有记忆库</button>
        </div>
        <div class="repository-heading"><h2>Git 仓库</h2><button type="button" class="text-button" :disabled="loading || saving" @click="emit('addRepository')"><Plus :size="15" />接入仓库</button></div>
        <div v-for="repo in workspace.repositories" :key="repo.id" class="repository-setting" @contextmenu.prevent="emit('repositoryMenu', $event, repo)">
          <GitBranch :size="16" /><div><h3>{{ repo.name }}</h3><p>{{ repo.remote }}</p></div><span>{{ repo.memoryCount }} 条记忆</span>
        </div>
        <p v-if="!workspace.repositories.length" class="settings-empty">暂无接入的仓库</p>
      </div>
      <aside class="settings-side-panel">
        <h2>本地文件</h2>
        <button type="button" class="button" :disabled="loading || saving" @click="emit('refresh')"><RefreshCw :size="15" :class="{ spinning: loading }" />刷新记忆库</button>
      </aside>
    </section>

    <div v-show="tab === 'update'"><AppUpdate /></div>
    <div v-if="tab === 'theme'" class="settings-embedded"><slot name="theme" :back="() => chooseTab('models')" /></div>
    <div v-if="tab === 'transfer'" class="settings-embedded"><slot name="transfer" :back="() => chooseTab('models')" /></div>

    <section v-if="settings" v-show="tab === 'archive'" class="settings-content-grid" data-testid="archive-settings">
      <div class="settings-main-panel">
        <header class="settings-section-title"><Archive :size="18" /><h2>自动归档</h2></header>
        <div class="settings-directory">
          <div class="settings-field-heading"><h3>Codex 数据目录</h3><button type="button" class="text-button" :disabled="saving || loading" @click="pickCodexDirectory"><FolderOpen :size="15" />选择目录</button></div>
          <p class="settings-path" data-testid="codex-directory">{{ draft.codexDirectory }}</p>
        </div>
        <div class="settings-directory">
          <div class="settings-field-heading"><h3>DeepSeek Harness 数据目录</h3><button type="button" class="text-button" :disabled="saving || loading" @click="pickHarnessDirectory"><FolderOpen :size="15" />选择目录</button></div>
          <p class="settings-path" data-testid="harness-directory">{{ draft.harnessDirectory || '尚未配置' }}</p>
        </div>
        <p v-if="directoryError" class="inline-error" role="alert">{{ directoryError }}</p>
        <fieldset class="archive-source-options" :disabled="saving || loading">
          <legend>归档来源</legend>
          <div><label><input v-model="draft.archiveSources" type="checkbox" value="codex" data-testid="archive-source-codex" />Codex</label><label><input v-model="draft.archiveSources" type="checkbox" value="harness" data-testid="archive-source-harness" />DeepSeek Harness</label></div>
        </fieldset>
        <div class="settings-option-row">
          <div><h3>按需求或功能点归档</h3></div>
          <label class="toggle-label"><input v-model="draft.archiveEnabled" type="checkbox" role="switch" :disabled="saving || loading" data-testid="archive-enabled" /><span>{{ draft.archiveEnabled ? '已启用' : '已暂停' }}</span></label>
        </div>
        <div class="settings-option-row">
          <div><h3>关闭窗口后驻留托盘</h3></div>
          <label class="toggle-label"><input v-model="draft.runInBackground" type="checkbox" role="switch" :disabled="saving || loading" data-testid="run-in-background" /><span>{{ draft.runInBackground ? '已开启' : '已关闭' }}</span></label>
        </div>
      </div>
      <aside class="settings-side-panel">
        <h2 class="settings-side-heading"><Clock3 :size="15" />归档起点</h2>
        <p class="archive-origin">{{ settings.archiveStartedAt ? dateTime(settings.archiveStartedAt) : '首次启用并保存时记录' }}</p>
      </aside>
    </section>

    <McpConnection v-show="tab === 'mcp'" :workspace-root="workspace.root" />
  </section>
</template>

<style scoped>
.integration-settings { scrollbar-gutter: stable; }
.settings-save { position: relative; }
.settings-save .button { min-inline-size: 118px; }
.settings-save-placeholder { visibility: hidden; }
.settings-save .draft-state { position: absolute; top: calc(100% + 6px); right: 0; margin: 0; line-height: 1.4; white-space: nowrap; }
.settings-tabs > button, .settings-tabs > button.active { font-weight: 500; }
.archive-source-options { margin: 18px 0 0; padding: 0 0 18px; border: 0; border-bottom: 1px solid var(--line); }
.archive-source-options legend { padding: 0; margin-bottom: 12px; font-size: var(--font-size-body); font-weight: 550; }
.archive-source-options > div { display: flex; flex-wrap: wrap; gap: 12px 24px; }
.archive-source-options label { display: flex; align-items: center; gap: 8px; font-size: var(--font-size-label); }
.archive-source-options input { accent-color: var(--accent); }
</style>
