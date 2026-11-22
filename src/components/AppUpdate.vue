<script lang="ts">
import { ref } from 'vue';

// 下载可以在切换页面后继续，返回设置时仍可查看保存结果。
const downloading = ref(false);
const savedPath = ref('');
const downloadError = ref('');
</script>

<script setup lang="ts">
import { computed } from 'vue';
import { Download, ExternalLink, FolderOpen, RefreshCw } from 'lucide-vue-next';
import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';
import { version } from '../../package.json';
import { checkAppUpdate, downloadAppUpdate } from '../api';
import type { AppUpdateResult } from '../types';

const checking = ref(false);
const busy = computed(() => checking.value || downloading.value);
const token = ref('');
let downloadToken: string | null = null;
const showToken = ref(false);
const result = ref<AppUpdateResult | null>(null);
const error = ref('');
const openError = ref('');

async function check() {
  if (busy.value) return;
  checking.value = true;
  result.value = null;
  error.value = '';
  openError.value = '';
  downloadError.value = '';
  savedPath.value = '';
  downloadToken = null;
  const accessToken = showToken.value ? token.value.trim() || null : null;
  token.value = '';
  try {
    result.value = await checkAppUpdate(accessToken);
    if (result.value.installer) downloadToken = accessToken;
    showToken.value = false;
  } catch (cause) {
    const failure = cause as { kind?: string; message?: string };
    error.value = failure?.message || String(cause);
    if (failure?.kind === 'access') showToken.value = true;
  } finally {
    checking.value = false;
  }
}

async function download() {
  const installer = result.value?.installer;
  if (busy.value || !installer) return;
  downloading.value = true;
  downloadError.value = '';
  openError.value = '';
  try {
    const path = await downloadAppUpdate(installer, downloadToken);
    if (path) {
      savedPath.value = path;
      downloadToken = null;
    }
  } catch (cause) {
    const failure = cause as { kind?: string; message?: string };
    downloadError.value = failure?.message || String(cause);
    if (failure?.kind === 'access') {
      downloadToken = null;
      showToken.value = true;
      result.value = null;
    }
  } finally {
    downloading.value = false;
  }
}

async function showFile() {
  openError.value = '';
  try {
    await revealItemInDir(savedPath.value);
  } catch (cause) {
    openError.value = `无法显示安装包：${String(cause)}`;
  }
}

async function openRelease() {
  openError.value = '';
  try {
    await openUrl(result.value?.releaseUrl ?? 'https://github.com/guo-straw/AiMemory/releases/latest');
  } catch (cause) {
    openError.value = `无法打开发布页：${String(cause)}`;
  }
}
</script>

<template>
  <section class="settings-content-grid app-update" aria-labelledby="app-update-heading">
    <div class="settings-main-panel">
      <header class="settings-section-title"><RefreshCw :size="18" /><h2 id="app-update-heading">应用更新</h2></header>
      <dl class="update-version"><dt>当前版本</dt><dd>{{ version }}</dd></dl>
      <div class="update-actions">
        <button v-if="downloading || (result?.installer && !savedPath)" type="button" class="button primary" :disabled="busy" data-testid="download-app-update" @click="download"><RefreshCw v-if="downloading" :size="15" class="spinning" /><Download v-else :size="15" />{{ downloading ? '正在下载…' : '下载安装包' }}</button>
        <button v-if="savedPath" type="button" class="button" @click="showFile"><FolderOpen :size="15" />在文件夹中显示</button>
        <button type="button" :class="result?.installer ? 'button' : 'button primary'" :disabled="busy" data-testid="check-app-update" @click="check"><RefreshCw :size="15" :class="{ spinning: checking }" />{{ checking ? '正在检查…' : '检查更新' }}</button>
        <button v-if="result?.status === 'available' || error || downloadError" type="button" class="text-button" @click="openRelease"><ExternalLink :size="14" />打开发布页</button>
      </div>
      <div class="update-result" role="status">
        <p v-if="savedPath" class="path-text">安装包已保存至 {{ savedPath }}</p>
        <template v-else-if="result?.status === 'available'">
          <p>发现新版本 {{ result.latestVersion }}。</p>
          <p v-if="result.installer" class="path-text">{{ result.installer.name }} · {{ (result.installer.size / 1024 / 1024).toFixed(1) }} MiB</p>
          <p v-else>这个版本没有适用于当前平台的安装包，可前往发布页查看。</p>
        </template>
        <p v-else-if="result?.status === 'current'">已是最新正式版。</p>
        <p v-else-if="result?.status === 'ahead'">当前版本高于最新正式版 {{ result.latestVersion }}。</p>
      </div>
      <div v-if="error" class="inline-error" role="alert">{{ error }}</div>
      <div v-if="downloadError" class="inline-error" role="alert">{{ downloadError }}</div>
      <div v-if="openError" class="inline-error" role="alert">{{ openError }}</div>
      <div class="update-access">
        <button type="button" class="text-button token-toggle" :aria-expanded="showToken" aria-controls="release-token" :disabled="busy" @click="showToken = !showToken; token = ''">使用 GitHub 令牌</button>
        <label v-if="showToken" id="release-token" class="field">GitHub 令牌（仅用于本次检查和下载）
          <input v-model="token" type="password" autocomplete="off" spellcheck="false" :disabled="busy" placeholder="需有此仓库的 Contents 只读权限" @keydown.enter.prevent="check" />
        </label>
      </div>
    </div>
    <aside class="settings-side-panel">
      <h2>发布来源</h2>
      <code class="update-repository">guo-straw/AiMemory</code>
    </aside>
  </section>
</template>

<style scoped>
.update-version { display: flex; align-items: baseline; justify-content: space-between; gap: 16px; padding: 20px 0; margin: 0; border-bottom: 1px solid var(--line); font-size: var(--font-size-body); }
.update-version dt { color: var(--muted); }
.update-version dd { margin: 0; font: 13px Menlo, Consolas, monospace; }
.update-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; margin-top: 18px; }
.update-result { color: var(--muted); font-size: var(--font-size-label); line-height: 1.8; }
.update-result > p { margin-top: 12px; }
.update-result > p + p { margin-top: 5px; }
.update-access { margin-top: 20px; padding-top: 15px; border-top: 1px solid var(--line); }
.token-toggle { padding: 0; font-size: var(--font-size-label); color: var(--accent-text); }
.update-access .field { margin-top: 12px; }
.update-repository { display: block; font: 12px/1.8 Menlo, Consolas, monospace; color: var(--fg); overflow-wrap: anywhere; }
</style>
