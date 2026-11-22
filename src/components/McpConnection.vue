<script setup lang="ts">
import { ref, watch } from 'vue';
import { Copy, Play, RefreshCw, Square, Terminal } from 'lucide-vue-next';
import { mcpHttpStart, mcpHttpStatus, mcpHttpStop, mcpInfo } from '../api';
import type { McpHttpStatus, McpInfo } from '../types';

const props = defineProps<{ workspaceRoot: string }>();
const info = ref<McpInfo | null>(null);
const status = ref<McpHttpStatus | null>(null);
const loading = ref(false);
const changing = ref(false);
const error = ref('');
const copyState = ref('');
const addressField = ref<HTMLInputElement>();

watch(() => props.workspaceRoot, refresh, { immediate: true });

async function refresh() {
  loading.value = true;
  error.value = '';
  try {
    status.value = await mcpHttpStatus();
    info.value = await mcpInfo();
  } catch (reason) {
    error.value = String(reason);
  } finally {
    loading.value = false;
  }
}

async function toggleService() {
  changing.value = true;
  error.value = '';
  copyState.value = '';
  try {
    status.value = await (status.value?.running ? mcpHttpStop() : mcpHttpStart());
  } catch (reason) {
    error.value = String(reason);
    status.value = await mcpHttpStatus().catch(() => null);
  } finally {
    changing.value = false;
  }
}

async function copy(text: string, field: HTMLInputElement | undefined, message: string) {
  try {
    await navigator.clipboard.writeText(text);
    copyState.value = message;
  } catch {
    field?.focus();
    field?.select();
    copyState.value = '请复制选中的内容';
  }
}
</script>

<template>
  <section class="settings-content-grid mcp-settings" data-testid="mcp-settings">
    <div class="mcp-page-heading">
      <header class="settings-section-title"><Terminal :size="18" /><h2>MCP 接入</h2></header>
    </div>

    <div class="settings-main-panel">
      <section class="mcp-http-panel" aria-labelledby="mcp-http-heading">
        <div class="mcp-http-heading">
          <div class="mcp-service-title">
            <h3 id="mcp-http-heading">本地 HTTP 服务</h3>
            <span class="mcp-status" :class="{ connected: status?.running }" role="status" data-testid="mcp-http-status">
              <i />{{ status?.running ? '运行中' : '未启动' }}
            </span>
          </div>
          <div class="mcp-service-actions">
            <button type="button" class="text-button" :disabled="loading || changing" @click="refresh">
              <RefreshCw :size="14" :class="{ spinning: loading }" />刷新状态
            </button>
            <button type="button" class="button" :class="{ primary: !status?.running }" :disabled="loading || changing || !status" @click="toggleService" data-testid="mcp-http-toggle">
              <Square v-if="status?.running" :size="13" /><Play v-else :size="13" />{{ changing ? (status?.running ? '正在停止…' : '正在启动…') : (status?.running ? '停止服务' : '启动服务') }}
            </button>
          </div>
        </div>

        <div class="mcp-address-field">
          <label class="field" for="mcp-http-url">连接地址</label>
          <div class="mcp-address-row">
            <input id="mcp-http-url" ref="addressField" :value="status?.url ?? ''" readonly spellcheck="false" aria-label="MCP HTTP 地址" data-testid="mcp-http-url" />
            <button type="button" class="button" :disabled="!status?.running || changing" @click="copy(status!.url, addressField, '地址已复制')" data-testid="copy-mcp-http-url">
              <Copy :size="14" />复制地址
            </button>
          </div>
        </div>
      </section>
      <p v-if="error" class="inline-error" role="alert">{{ error }}</p>

      <p class="mcp-copy-feedback" role="status" aria-live="polite">{{ copyState }}</p>
    </div>

    <aside class="settings-side-panel mcp-info-panel">
      <h2>接入信息</h2>
      <dl class="mcp-paths">
        <div><dt>传输协议</dt><dd>Streamable HTTP</dd></div>
        <div><dt>服务范围</dt><dd>仅当前电脑 · 127.0.0.1</dd></div>
        <template v-if="info">
          <div><dt>配置目录</dt><dd class="mcp-directory">{{ info.dataDirectory }}</dd></div>
          <div><dt>记忆库</dt><dd class="mcp-directory">{{ info.vaultRoot }}</dd></div>
        </template>
      </dl>
    </aside>
  </section>
</template>

<style scoped>
.mcp-settings { column-gap: 28px; row-gap: 22px; }
.mcp-page-heading { grid-column: 1 / -1; }
.mcp-http-panel { padding: 20px; border: 1px solid var(--line); border-radius: 10px; }
.mcp-http-heading { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 16px; padding-bottom: 18px; border-bottom: 1px solid var(--line); }
.mcp-service-title { display: grid; gap: 9px; }
.mcp-service-title h3 { font-size: var(--font-size-body); font-weight: 600; }
.mcp-status { display: inline-flex; align-items: center; gap: 7px; color: var(--muted); font-size: var(--font-size-label); line-height: 1.5; }
.mcp-status i { width: 6px; height: 6px; flex-shrink: 0; border-radius: 50%; background: var(--faint); }
.mcp-status.connected { color: var(--accent-text); }
.mcp-status.connected i { background: var(--accent); }
.mcp-service-actions { display: flex; align-items: center; gap: 10px; }
.mcp-service-actions .text-button, .mcp-settings .button { font-size: var(--font-size-label); }
.mcp-address-field { display: grid; gap: 9px; margin-top: 18px; }
.mcp-address-row { display: flex; align-items: stretch; gap: 8px; }
.mcp-address-row input { flex: 1; width: 0; min-width: 0; padding: 10px 12px; border: 1px solid var(--line); border-radius: 7px; background: var(--side); color: var(--fg); font: 12px/1.6 Menlo, Consolas, monospace; }
.mcp-copy-feedback { min-height: 18px; margin-top: 10px; color: var(--accent-text); font-size: var(--font-size-label); line-height: 1.5; }
.mcp-info-panel .mcp-paths { gap: 16px; }
.mcp-info-panel .mcp-paths dd { font: inherit; color: var(--fg); }
.mcp-info-panel .mcp-paths .mcp-directory { font: 11px/1.8 Menlo, Consolas, monospace; }
@media (max-width: 700px) {
  .mcp-http-panel { padding: 18px; }
}
@media (max-width: 480px) {
  .mcp-service-actions { width: 100%; justify-content: space-between; }
  .mcp-address-row { flex-direction: column; }
  .mcp-address-row input { width: 100%; flex: auto; }
}
</style>
