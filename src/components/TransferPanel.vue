<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue';
import { ArrowLeft, ArrowRight, BookOpen, Check, ChevronDown, ChevronLeft, ChevronRight, Download, FileJson, FileText, Info, Link2, RefreshCw, Settings2, Upload } from 'lucide-vue-next';
import type { AppSettings, Workspace } from '../types';
import type { ThemeConfig } from '../theme';
import { chooseImport, exportTransfer, transferApply, transferDiscard } from '../transfer';
import type { ImportPreview, TransferApplyResult } from '../transfer';

const props = defineProps<{ workspace: Workspace; settings: AppSettings | null; theme: ThemeConfig; embedded?: boolean }>();
const emit = defineEmits<{ back: []; imported: [result: TransferApplyResult] }>();
const preview = ref<ImportPreview | null>(null);
const previewExpired = ref(false);
const operation = ref<'export' | 'select' | 'apply' | 'cancel' | null>(null);
const error = ref('');
const message = ref('');
const exportPath = ref('');
const memoryPage = ref(0);
const linkPage = ref(0);
const expandedMemoryId = ref<string | null>(null);
const pageSize = 50;
const busy = computed(() => operation.value !== null);
const memoryCount = computed(() => props.workspace.repositories.reduce((sum, repo) => sum + repo.memoryCount, 0));
const filename = computed(() => preview.value?.path.split(/[\\/]/).pop() ?? '');
const visibleMemories = computed(() => preview.value?.memories.slice(memoryPage.value * pageSize, (memoryPage.value + 1) * pageSize) ?? []);
const visibleLinks = computed(() => preview.value?.links.slice(linkPage.value * pageSize, (linkPage.value + 1) * pageSize) ?? []);
const memoryPages = computed(() => Math.ceil((preview.value?.memories.length ?? 0) / pageSize));
const linkPages = computed(() => Math.ceil((preview.value?.links.length ?? 0) / pageSize));

async function exportPackage(kind: 'memories' | 'config') {
  if (busy.value) return;
  operation.value = 'export';
  error.value = '';
  message.value = '';
  exportPath.value = '';
  try {
    const path = await exportTransfer(kind);
    if (path) {
      exportPath.value = path;
      message.value = `已导出${kind === 'memories' ? '记忆' : '配置'}包`;
    }
  } catch (cause) {
    error.value = String(cause);
  } finally {
    operation.value = null;
  }
}

async function selectPackage(kind: 'memories' | 'config') {
  if (busy.value) return;
  operation.value = 'select';
  error.value = '';
  message.value = '';
  exportPath.value = '';
  try {
    const next = await chooseImport(kind);
    if (!next) return;
    if (preview.value) await transferDiscard(preview.value.token);
    preview.value = next;
    previewExpired.value = false;
    memoryPage.value = 0;
    linkPage.value = 0;
    expandedMemoryId.value = null;
  } catch (cause) {
    error.value = String(cause);
  } finally {
    operation.value = null;
  }
}

async function cancelPreview() {
  if (busy.value || !preview.value) return;
  operation.value = 'cancel';
  error.value = '';
  try {
    await transferDiscard(preview.value.token);
    preview.value = null;
    previewExpired.value = false;
    message.value = '已取消导入';
  } catch (cause) {
    error.value = String(cause);
  } finally {
    operation.value = null;
  }
}

async function applyPreview() {
  if (busy.value || !preview.value || previewExpired.value) return;
  operation.value = 'apply';
  error.value = '';
  try {
    const result = await transferApply(preview.value.token);
    if (result.kind === 'memories') {
      message.value = `已新增 ${result.addedMemories} 条记忆、${result.newLinks} 对关联，保留本地 ${result.existingMemories} 条记忆`;
    } else message.value = '已应用导入的配置';
    preview.value = null;
    previewExpired.value = false;
    emit('imported', result);
  } catch (cause) {
    previewExpired.value = true;
    error.value = String(cause);
  } finally {
    operation.value = null;
  }
}

function back() {
  if (busy.value) return;
  if (preview.value) void cancelPreview();
  else emit('back');
}

function linkStatus(status: 'new' | 'existing' | 'invalid') {
  return status === 'new' ? '新增关联' : status === 'existing' ? '已存在' : '端点无效，跳过';
}

onBeforeUnmount(() => {
  if (preview.value) void transferDiscard(preview.value.token).catch(() => {});
});
defineExpose({ busy });
</script>

<template>
  <section class="transfer-page" :aria-busy="busy" data-testid="transfer-page">
    <button v-if="!embedded || preview" type="button" class="transfer-back text-button" :disabled="busy" @click="back"><ArrowLeft :size="16" />{{ preview ? '返回导入与导出' : '返回设置' }}</button>
    <header v-if="!embedded || preview" class="transfer-heading">
      <component :is="embedded ? 'h2' : 'h1'">{{ preview ? preview.kind === 'memories' ? '记忆导入预览' : '配置导入预览' : '记忆与配置迁移' }}</component>
    </header>

    <p v-if="error" class="inline-error transfer-error" role="alert" data-testid="transfer-error">{{ error }}</p>
    <div v-if="message && !preview" class="transfer-message" role="status" data-testid="transfer-result"><Check :size="16" /><div><span>{{ message }}</span><p v-if="exportPath" class="transfer-path" data-testid="transfer-export-path">{{ exportPath }}</p></div></div>

    <div v-if="!preview" class="transfer-layout">
      <div class="transfer-packages">
        <section class="package-section" aria-labelledby="memory-package-title">
          <span class="package-mark"><BookOpen :size="20" /></span>
          <div class="package-body">
            <h2 id="memory-package-title">记忆包</h2>
            <code>knowtrace-memories.json</code>
            <div class="package-actions"><button type="button" class="button primary" :disabled="busy" data-testid="export-memories" @click="exportPackage('memories')"><Download :size="15" />导出记忆</button><button type="button" class="button" :disabled="busy" data-testid="import-memories" @click="selectPackage('memories')"><Upload :size="15" />导入记忆</button></div>
          </div>
        </section>
        <section class="package-section" aria-labelledby="config-package-title">
          <span class="package-mark"><Settings2 :size="20" /></span>
          <div class="package-body">
            <h2 id="config-package-title">配置包</h2>
            <code>knowtrace-config.json</code>
            <div class="package-actions"><button type="button" class="button primary" :disabled="busy || !settings" data-testid="export-config" @click="exportPackage('config')"><Download :size="15" />导出配置</button><button type="button" class="button" :disabled="busy || !settings" data-testid="import-config" @click="selectPackage('config')"><Upload :size="15" />导入配置</button></div>
          </div>
        </section>
      </div>
      <aside class="transfer-summary" aria-labelledby="transfer-summary-title">
        <h2 id="transfer-summary-title">当前工作区</h2>
        <dl class="transfer-context">
          <div><dt>Git 仓库</dt><dd>{{ workspace.repositories.length }} 个</dd></div>
          <div><dt>记忆</dt><dd>{{ memoryCount }} 条</dd></div>
          <div><dt>主题</dt><dd>{{ theme.preset }}</dd></div>
        </dl>
        <div class="transfer-format"><FileJson :size="16" /><span>UTF-8 JSON · 版本 1</span></div>
        <p v-if="busy" class="transfer-progress" role="status"><RefreshCw :size="13" class="spinning" />正在处理…</p>
      </aside>
    </div>

    <template v-else>
      <div class="import-source"><FileJson :size="23" /><div><strong data-testid="import-file-name">{{ filename }}</strong></div><span>UTF-8 JSON</span><button type="button" class="text-button" :disabled="busy" data-testid="transfer-reselect" @click="selectPackage(preview.kind === 'memories' ? 'memories' : 'config')">重新选择</button></div>
      <p v-if="previewExpired" class="preview-expired" role="status" data-testid="transfer-preview-expired">预览已失效，请重新选择文件。</p>

      <template v-if="preview.kind === 'memories'">
        <div class="import-counts" data-testid="memory-import-counts"><div><span>新增记忆</span><strong>{{ preview.addedMemories }}</strong></div><div><span>已存在</span><strong>{{ preview.existingMemories }}</strong></div><div><span>新增关联</span><strong>{{ preview.newLinks }}</strong></div></div>
        <div class="preview-section-heading"><h2>条目明细</h2><span>共 {{ preview.memories.length }} 条记忆</span></div>
        <div class="transfer-table-scroll"><table class="memory-import-table"><thead><tr><th scope="col">记忆</th><th scope="col">仓库</th><th scope="col">记忆 ID</th><th scope="col">处理方式</th></tr></thead><tbody><template v-for="row in visibleMemories" :key="row.id"><tr :data-memory-id="row.id"><td><button type="button" class="memory-preview-toggle" :aria-expanded="expandedMemoryId === row.id" :aria-controls="`import-memory-${row.id}`" @click="expandedMemoryId = expandedMemoryId === row.id ? null : row.id"><FileText :size="16" /><span>{{ row.title }}</span><ChevronDown :size="14" :class="{ expanded: expandedMemoryId === row.id }" /></button></td><td class="repository-cell"><span>{{ row.repository }}</span><small v-if="row.localRepository !== null && row.localRepository !== row.repository">本地保留 · {{ row.localRepository }}</small></td><td class="id-cell">{{ row.id }}</td><td><span class="import-badge" :class="{ existing: row.status === 'existing' }">{{ row.status === 'new' ? '新增' : '保留本地' }}</span></td></tr><tr v-if="expandedMemoryId === row.id" :id="`import-memory-${row.id}`" class="memory-preview-content"><td colspan="4"><h3>文件中的 Markdown</h3><pre>{{ row.markdown }}</pre><template v-if="row.localMarkdown !== null"><h3>本地保留的 Markdown <span v-if="row.localRepository">· {{ row.localRepository }}</span></h3><pre>{{ row.localMarkdown }}</pre></template></td></tr></template><tr v-if="!preview.memories.length"><td colspan="4" class="empty-preview">文件中没有记忆条目。</td></tr></tbody></table></div>
        <nav v-if="memoryPages > 1" class="preview-pagination" aria-label="记忆条目分页"><button type="button" class="button" :disabled="memoryPage === 0" @click="memoryPage--; expandedMemoryId = null"><ChevronLeft :size="14" />上一页</button><span>{{ memoryPage + 1 }} / {{ memoryPages }}</span><button type="button" class="button" :disabled="memoryPage + 1 === memoryPages" @click="memoryPage++; expandedMemoryId = null">下一页<ChevronRight :size="14" /></button></nav>
        <section class="import-links" aria-labelledby="import-links-title"><div class="preview-section-heading"><h2 id="import-links-title"><Link2 :size="18" />关联关系</h2><span>{{ preview.newLinks }} 对新增<span v-if="preview.invalidLinks"> · {{ preview.invalidLinks }} 对无效</span></span></div><div v-for="(link, position) in visibleLinks" :key="`${link.from}:${link.to}:${position}`" class="import-link-row"><code>{{ link.from }}</code><span aria-hidden="true">↔</span><code>{{ link.to }}</code><span class="link-status" :class="link.status">{{ linkStatus(link.status) }}</span></div><p v-if="!preview.links.length" class="empty-preview">文件中没有需要合并的关联。</p><nav v-if="linkPages > 1" class="preview-pagination" aria-label="关联分页"><button type="button" class="button" :disabled="linkPage === 0" @click="linkPage--"><ChevronLeft :size="14" />上一页</button><span>{{ linkPage + 1 }} / {{ linkPages }}</span><button type="button" class="button" :disabled="linkPage + 1 === linkPages" @click="linkPage++">下一页<ChevronRight :size="14" /></button></nav></section>
      </template>

      <template v-else>
        <div class="preview-section-heading config-differences"><h2>设置差异</h2><span class="change-count">{{ preview.changedCount }} 项变更</span></div>
        <div class="transfer-table-scroll"><table class="config-import-table"><thead><tr><th scope="col">配置项</th><th scope="col">此设备</th><th scope="col">导入后</th></tr></thead><tbody><tr v-for="row in preview.rows" :key="row.label" :class="{ changed: row.changed }"><th scope="row">{{ row.label }}</th><td>{{ row.before }}</td><td><ArrowRight v-if="row.changed" :size="15" aria-hidden="true" /><span>{{ row.after }}</span><small v-if="!row.changed">沿用</small></td></tr></tbody></table></div>
        <div class="config-preservation"><Info :size="17" /><div><p>API 密钥、记忆库路径和归档游标继续使用本机值。</p><p v-if="!preview.hasTheme">此配置包没有主题设置，沿用当前外观。</p></div></div>
      </template>

      <footer class="transfer-confirm"><p>{{ preview.kind === 'memories' ? '相同 ID 保留本地版本。' : `确认后应用上方 ${preview.changedCount} 项配置变更。` }}</p><div><button type="button" class="button" :disabled="busy" data-testid="transfer-cancel" @click="cancelPreview">取消</button><button type="button" class="button primary" :disabled="busy || previewExpired" data-testid="transfer-confirm" @click="applyPreview"><RefreshCw v-if="operation === 'apply'" :size="16" class="spinning" /><Check v-else :size="16" />{{ operation === 'apply' ? '正在导入…' : '确认导入' }}</button></div></footer>
    </template>
  </section>
</template>

<style scoped>
.transfer-page { flex: 1; min-width: 0; min-height: 0; overflow-y: auto; padding: 30px 44px; }
.transfer-back { margin: 0 0 18px -7px; color: var(--accent-text); }
.transfer-heading { max-width: 1100px; margin-bottom: 20px; }
.transfer-heading > :is(h1, h2) { font-size: var(--font-size-page); font-weight: 600; line-height: 1.5; }
.transfer-heading > p { color: var(--muted); font-size: var(--font-size-label); line-height: 1.7; margin-top: 7px; }
.transfer-error { max-width: 1100px; padding: 0 0 12px; }
.transfer-message { display: flex; align-items: flex-start; gap: 9px; max-width: 1100px; padding: 12px 15px; margin-bottom: 20px; background: var(--soft); color: var(--accent-text); border: 1px solid var(--line); border-radius: 8px; font-size: var(--font-size-body); line-height: 1.65; }
.transfer-message > svg { flex-shrink: 0; margin-top: 2px; }
.transfer-path { color: var(--muted); font: 11px/1.7 Menlo, Consolas, monospace; margin-top: 5px; overflow-wrap: anywhere; }
.transfer-layout { display: grid; grid-template-columns: minmax(420px, 1.2fr) minmax(280px, 1fr); align-items: start; gap: 36px; max-width: 1100px; }
.transfer-packages { min-width: 0; }
.package-section { display: flex; gap: 14px; padding-bottom: 24px; }
.package-section + .package-section { border-top: 1px solid var(--line); padding-top: 24px; }
.package-mark { display: grid; place-items: center; width: 36px; height: 36px; border-radius: 8px; background: var(--soft); color: var(--accent); flex-shrink: 0; }
.package-body { flex: 1; min-width: 0; }
.package-body h2 { font-size: var(--font-size-section); font-weight: 600; line-height: 1.5; }
.package-body > p { color: var(--muted); font-size: var(--font-size-label); line-height: 1.8; margin-top: 7px; }
.package-body > code { display: block; font: 11px/1.7 Menlo, Consolas, monospace; color: var(--muted); margin-top: 9px; overflow-wrap: anywhere; }
.package-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; margin-top: 16px; }
.transfer-summary { min-width: 0; padding: 21px; border: 1px solid var(--line); border-radius: 10px; background: var(--side); }
.transfer-summary > h2 { font-size: var(--font-size-body); font-weight: 550; margin: 0 0 18px; }
.transfer-context { display: grid; gap: 13px; margin: 0; font-size: var(--font-size-label); }
.transfer-context > div { display: flex; align-items: baseline; justify-content: space-between; gap: 16px; }
.transfer-context dt { color: var(--muted); }
.transfer-context dd { margin: 0; text-align: right; overflow-wrap: anywhere; }
.transfer-format { display: flex; align-items: center; gap: 8px; border-top: 1px solid var(--line); margin-top: 19px; padding-top: 16px; color: var(--muted); font-size: var(--font-size-label); }
.transfer-format > svg { flex-shrink: 0; }
.transfer-progress { display: flex; align-items: center; gap: 7px; margin-top: 16px; color: var(--muted); font-size: var(--font-size-label); }
.import-source { display: flex; align-items: center; gap: 12px; border-radius: 8px; padding: 12px 15px; background: var(--side); margin-top: 16px; }
.import-source > svg { flex-shrink: 0; color: var(--accent); }
.import-source > div { flex: 1; min-width: 0; }
.import-source strong { display: block; font: 500 14px/1.6 Menlo, Consolas, monospace; overflow-wrap: anywhere; }
.import-source p { color: var(--muted); font-size: 11px; line-height: 1.6; margin-top: 4px; }
.import-source > span { color: var(--muted); font: 11px Menlo, Consolas, monospace; white-space: nowrap; }
.import-source > .text-button { font-size: 11px; color: var(--accent-text); }
.preview-expired { margin-top: 14px; color: var(--muted); font-size: 12px; line-height: 1.8; }
.import-counts { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); margin-top: 18px; padding: 1px 0; }
.import-counts > div { padding-left: 25px; }
.import-counts > div:first-child { padding-left: 0; }
.import-counts > div + div { border-left: 1px solid var(--line); }
.import-counts span { color: var(--muted); font-size: 13px; }
.import-counts strong { display: block; font-size: var(--font-size-page); line-height: 1.5; font-weight: 600; font-variant-numeric: tabular-nums; }
.import-counts > div:first-child strong { color: var(--accent-text); }
.preview-section-heading { display: flex; align-items: center; flex-wrap: wrap; justify-content: space-between; gap: 12px; margin: 20px 0 12px; }
.preview-section-heading h2 { display: flex; align-items: center; gap: 9px; font-size: var(--font-size-section); font-weight: 600; }
.preview-section-heading > span { color: var(--muted); font-size: 12px; line-height: 1.7; }
.transfer-table-scroll { overflow-x: auto; border: 1px solid var(--line); border-radius: 8px; }
.transfer-page table { border-collapse: collapse; text-align: left; width: 100%; font-size: 12px; }
.transfer-page thead { background: var(--side); color: var(--muted); font-size: 11px; }
.transfer-page th { font-weight: 500; }
.transfer-page thead th { padding: 9px 12px; white-space: nowrap; }
.transfer-page td, .config-import-table tbody th { border-bottom: 1px solid var(--line); padding: 10px 12px; vertical-align: top; line-height: 1.7; }
.memory-import-table { min-width: 600px; }
.memory-import-table th:first-child { width: 45%; }
.memory-import-table th:nth-child(2) { width: 27%; }
.memory-import-table th:nth-child(3) { width: 16%; }
.memory-import-table th:nth-child(4) { width: 12%; }
.memory-preview-toggle { display: flex; align-items: flex-start; gap: 9px; border: 0; padding: 0; background: transparent; text-align: left; width: 100%; font-size: 13px; line-height: 1.7; }
.memory-preview-toggle > svg { margin-top: 3px; color: var(--muted); flex-shrink: 0; }
.memory-preview-toggle > span { overflow-wrap: anywhere; flex: 1; }
.memory-preview-toggle > svg:last-child { margin-left: auto; transition: transform .13s; }
.memory-preview-toggle > svg.expanded { transform: rotate(180deg); }
.repository-cell { overflow-wrap: anywhere; color: var(--muted); }
.repository-cell > small { display: block; font-size: 10px; margin-top: 5px; color: var(--faint); }
.id-cell { font: 11px/1.8 Menlo, Consolas, monospace; overflow-wrap: anywhere; }
.import-badge { display: inline-block; white-space: nowrap; padding: 3px 9px; border-radius: 6px; background: var(--soft); color: var(--accent-text); font-size: 11px; font-weight: 550; }
.import-badge.existing { background: var(--side); color: var(--muted); }
.memory-preview-content td { padding: 15px 18px 21px; background: var(--side); }
.memory-preview-content h3 { font-size: 12px; font-weight: 550; }
.memory-preview-content h3 > span { color: var(--muted); font-weight: 400; }
.memory-preview-content pre { max-height: 280px; overflow: auto; margin: 11px 0 19px; padding: 13px; border: 1px solid var(--line); border-radius: 6px; background: var(--bg, #fff); white-space: pre-wrap; overflow-wrap: anywhere; font: 11px/1.8 Menlo, Consolas, monospace; }
.memory-preview-content pre:last-child { margin-bottom: 0; }
.preview-pagination { display: flex; align-items: center; justify-content: flex-end; gap: 13px; margin-top: 15px; color: var(--muted); font: 11px Menlo, Consolas, monospace; }
.preview-pagination .button { font: 11px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; min-height: 31px; padding: 6px 10px; }
.import-links { margin-top: 20px; }
.import-links h2 > svg { color: var(--accent); }
.import-link-row { display: flex; align-items: center; gap: 11px; flex-wrap: wrap; padding: 10px 14px; border-bottom: 1px solid var(--line); font-size: 12px; }
.import-link-row > code { font: 11px Menlo, Consolas, monospace; overflow-wrap: anywhere; }
.import-link-row > span:not(.link-status) { color: var(--faint); }
.link-status { margin-left: auto; color: var(--muted); font-size: 11px; }
.link-status.new { color: var(--accent-text); }
.link-status.invalid { color: var(--faint); }
.empty-preview { color: var(--faint); padding: 15px 0; font-size: 12px; line-height: 1.8; }
.config-differences { margin-top: 20px; justify-content: flex-start; }
.preview-section-heading > .change-count { padding: 3px 10px; border-radius: 6px; color: var(--accent-text); background: var(--soft); font-size: 12px; font-weight: 550; }
.config-import-table { table-layout: fixed; }
.config-import-table th { width: 29%; }
.config-import-table tbody th { font-size: 13px; }
.config-import-table td { overflow-wrap: anywhere; color: var(--muted); padding-top: 11px; padding-bottom: 11px; }
.config-import-table td:last-child > svg { display: inline-block; vertical-align: -3px; margin-right: 9px; color: var(--faint); }
.config-import-table td:last-child > small { display: block; font-size: 10px; margin-top: 4px; color: var(--faint); }
.config-import-table .changed td:last-child > span { color: var(--accent-text); font-weight: 550; }
.config-preservation { display: flex; align-items: flex-start; gap: 10px; color: var(--muted); margin-top: 18px; }
.config-preservation > svg { flex-shrink: 0; margin-top: 2px; }
.config-preservation p { font-size: 13px; line-height: 1.8; }
.config-preservation p + p { color: var(--faint); font-size: 12px; margin-top: 7px; }
.transfer-confirm { display: flex; align-items: center; justify-content: space-between; gap: 20px; margin-top: 20px; padding: 16px 0 0; border-top: 1px solid var(--line); }
.transfer-confirm > p { max-width: 66%; font-size: 12px; line-height: 1.8; color: var(--muted); }
.transfer-confirm > div { display: flex; gap: 10px; flex-shrink: 0; }
@media (max-width: 1050px) { .transfer-page { padding-left: 30px; padding-right: 30px; } .transfer-layout { grid-template-columns: minmax(0, 1fr); max-width: 680px; gap: 24px; } }
@media (max-width: 700px) {
  .transfer-page { padding: 20px 24px 26px; }
  .transfer-heading > p { font-size: 13px; }
  .package-section { gap: 12px; }
  .transfer-summary { padding: 18px; }
  .package-mark { width: 36px; height: 36px; border-radius: 8px; }
  .package-mark > svg { width: 18px; height: 18px; }
  .import-source { flex-wrap: wrap; gap: 10px; padding: 14px; }
  .import-source > span { display: none; }
  .import-source > .text-button { margin-left: auto; }
  .import-counts > div { padding-left: 14px; }
  .import-counts span { font-size: 11px; }
  .config-import-table th, .config-import-table td { padding-left: 8px; padding-right: 8px; }
  .config-import-table tbody th { font-size: 12px; }
  .config-import-table td { font-size: 11px; }
  .transfer-confirm { align-items: stretch; flex-direction: column; gap: 17px; }
  .transfer-confirm > p { max-width: none; }
  .transfer-confirm > div { justify-content: flex-end; }
}
@media (prefers-reduced-motion: reduce) { .memory-preview-toggle > svg:last-child { transition: none; } }
</style>
