<script setup lang="ts">
import { computed } from 'vue';
import { Activity, Archive, Clock3, GitBranch, RefreshCw } from 'lucide-vue-next';
import type { ArchiveStatus, ClientSource, MemorySummary } from '../types';
import { clientLabel } from '../usage';

const props = defineProps<{ status: ArchiveStatus | null; memories: MemorySummary[]; readError: string; retrying: boolean; embedded?: boolean; archiveSources?: ClientSource[] }>();
const emit = defineEmits<{ retry: []; settings: []; openMemory: [id: string] }>();
const archiveSourceLabel = computed(() => props.archiveSources === undefined ? 'Codex' : props.archiveSources.map(clientLabel).join('、') || '未选择来源');
const usage = computed(() => [
  { label: '聊天输入', value: props.status?.chatInputTokens ?? 0, color: '#3566F0' },
  { label: '聊天输出', value: props.status?.chatOutputTokens ?? 0, color: '#92ACFB' },
  { label: '向量输入', value: props.status?.embeddingInputTokens ?? 0, color: '#4E545F' },
]);
const totalTokens = computed(() => usage.value.reduce((sum, item) => sum + item.value, 0));
const hasFailure = computed(() => (props.status?.failedEmbeddings ?? 0) > 0 || props.status?.recentTurns.some(turn => turn.status === 'failed'));
const memoryTitles = computed(() => new Map(props.memories.map(memory => [memory.id, memory.title])));
const failedMemories = computed(() => props.status?.failedEmbeddingDetails ?? []);
const statusLabels = { pending: '等待提取', extracting: '正在提取', saved: '正文已保存', failed: '处理失败', skipped: '已跳过' };

function number(value: number) { return value.toLocaleString('zh-CN'); }
function dateTime(value: string) { return new Date(value).toLocaleString('zh-CN', { hour12: false }); }
function duration(value: number | null) { return value === null ? '—' : value < 1000 ? `${value} ms` : `${(value / 1000).toFixed(2)} s`; }
</script>

<template>
  <section v-if="readError" class="archive-page" data-testid="archive-page">
    <h1 v-if="!embedded">记忆归档</h1>
    <p class="archive-error" role="alert">读取归档状态失败。{{ readError }}</p>
  </section>
  <section v-else class="archive-page" data-testid="archive-page">
    <div class="page-heading">
      <div v-if="!embedded">
        <h1>记忆归档</h1>
      </div>
      <div class="archive-page-actions">
        <button v-if="hasFailure" class="button" :disabled="retrying" @click="emit('retry')" data-testid="retry-archive"><RefreshCw :size="14" :class="{ spinning: retrying }" />{{ retrying ? '正在重试…' : '重试失败任务' }}</button>
        <button class="button" @click="emit('settings')">归档设置</button>
      </div>
    </div>
    <div class="archive-state-line">
      <span :class="{ enabled: status?.enabled }">
        <i />{{ !status ? '正在读取归档状态…' : status.enabled ? '自动归档已启用' : '自动归档已暂停' }}
      </span>
      <span data-testid="archive-sources">归档来源：{{ archiveSourceLabel }}</span>
      <span v-if="status?.startedAt">{{ dateTime(status.startedAt) }} 起</span>
      <span v-if="status?.scanning" class="scan-state">
        <RefreshCw :size="13" class="spinning" />正在检查新轮次
      </span>
    </div>

    <div class="archive-metrics">
      <div>
        <span>已处理轮次</span>
        <strong data-testid="processed-turns">{{ number(status?.processedTurns ?? 0) }}<small>轮</small></strong>
      </div>
      <div>
        <span>已归档事项</span>
        <strong data-testid="written-memories">{{ number(status?.writtenMemories ?? 0) }}<small>条</small></strong>
      </div>
      <div>
        <span>等待向量索引</span>
        <strong data-testid="pending-embeddings">{{ number(status?.pendingEmbeddings ?? 0) }}<small>条</small></strong>
        <p v-if="status?.failedEmbeddings">另有 {{ status.failedEmbeddings }} 条记忆索引失败</p>
      </div>
    </div>

    <div v-if="status?.lastError" class="archive-error" role="alert">
      <span>{{ status.lastError }}</span>
      <button class="button" @click="emit('settings')">检查设置</button>
    </div>

    <section v-if="status?.failedEmbeddings" class="archive-index-failures" data-testid="failed-memory-indexes">
      <h2>{{ status.failedEmbeddings }} 条记忆索引生成失败</h2>
      <p>以下为已保存记忆的索引任务，已跳过的轮次不会生成索引。</p>
      <ul>
        <li v-for="memory in failedMemories" :key="memory.memoryId">
          <button type="button" class="text-button" @click="emit('openMemory', memory.memoryId)" :data-failed-memory-id="memory.memoryId">{{ memoryTitles.get(memory.memoryId) ?? memory.memoryId }}</button>
          <p>{{ memory.error ?? '未记录失败原因' }}</p>
        </li>
      </ul>
      <p v-if="status.failedEmbeddings > failedMemories.length">显示前 {{ failedMemories.length }} 条失败记录。</p>
    </section>

    <div class="archive-detail-grid">
      <section class="usage-panel">
        <header>
          <Activity :size="18" />
          <h2>记忆归档模型用量</h2>
        </header>
        <div class="usage-total">
          <strong>{{ number(totalTokens) }}</strong>
          <span>{{ status?.usageIncomplete ? '已知 tokens' : 'tokens' }}</span>
        </div>
        <div class="usage-bar" aria-hidden="true">
          <span v-for="item in usage" :key="item.label" :style="{ width: totalTokens ? `${item.value / totalTokens * 100}%` : '0%', background: item.color }" />
        </div>
        <div v-for="item in usage" :key="item.label" class="usage-row">
          <i :style="{ background: item.color }" />
          <span>{{ item.label }}</span>
          <strong>{{ number(item.value) }}</strong>
        </div>
        <p v-if="status?.usageIncomplete" class="usage-footnote">部分请求未返回用量</p>
      </section>
    </div>

    <section class="archive-log">
      <header>
        <h2>最近处理的轮次</h2>
      </header>
      <div v-if="status?.recentTurns.length" class="archive-table-scroll">
        <table>
          <thead>
            <tr>
              <th>完成时间</th>
              <th>仓库 / 来源</th>
              <th>本轮新增或更新</th>
              <th>聊天 / 向量耗时</th>
              <th>状态</th>
            </tr>
          </thead>
          <tbody>
            <template v-for="turn in status.recentTurns" :key="turn.sourceId">
              <tr :data-source-id="turn.sourceId" :data-client="turn.client ?? 'codex'">
                <td><Clock3 :size="13" />{{ dateTime(turn.completedAt) }}</td>
                <td class="archive-source">
                  <span><GitBranch :size="14" />{{ turn.repository }}</span>
                  <small :title="`${clientLabel(turn.client)} · ${turn.sessionId} · ${turn.turnId}`">{{ clientLabel(turn.client) }} · {{ turn.sessionId.slice(0, 8) }} · {{ turn.turnId }}</small>
                </td>
                <td>{{ turn.memoryCount }} 条</td>
                <td class="archive-latency">
                  {{ duration(turn.chatLatencyMs) }}
                  <small>{{ duration(turn.embeddingLatencyMs) }}</small>
                </td>
                <td>
                  <span class="archive-turn-status" :class="turn.status">{{ turn.status === 'saved' && turn.memoryCount === 0 ? '无新结论' : statusLabels[turn.status] }}</span>
                  <small v-if="turn.usageIncomplete && !turn.chatInputTokens && !turn.chatOutputTokens && !turn.embeddingInputTokens" class="turn-token-count">用量未返回</small>
                  <small v-else class="turn-token-count">{{ turn.usageIncomplete ? '已知 ' : '' }}{{ number(turn.chatInputTokens) }} / {{ number(turn.chatOutputTokens) }} / {{ number(turn.embeddingInputTokens) }} tokens</small>
                </td>
              </tr>
              <tr v-if="turn.error" :class="turn.status === 'skipped' ? 'turn-skip-row' : 'turn-error-row'">
                <td colspan="5">{{ turn.status === 'skipped' ? '跳过原因：' : '' }}{{ turn.error }}</td>
              </tr>
            </template>
          </tbody>
        </table>
      </div>
      <div v-else class="archive-log-empty">
        <Archive :size="26" />
        <h3>暂无归档记录</h3>
        <button v-if="!status?.enabled" class="button" @click="emit('settings')">配置自动归档</button>
      </div>
    </section>
  </section>
</template>

<style scoped>
.archive-page-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }
.archive-index-failures { padding: 18px 22px; margin-top: 22px; border: 1px solid var(--line); border-radius: 10px; }
.archive-index-failures h2 { color: var(--danger); font-size: 14px; font-weight: 600; }
.archive-index-failures p { color: var(--muted); font-size: 12px; line-height: 1.7; margin-top: 7px; overflow-wrap: anywhere; }
.archive-index-failures ul { margin: 12px 0 0; padding: 0; list-style: none; }
.archive-index-failures li + li { margin-top: 12px; padding-top: 12px; border-top: 1px solid var(--line); }
.archive-index-failures .text-button { padding: 0; text-align: left; font-size: 13px; }
.archive-log .turn-skip-row td { padding-top: 0; border-top: 0; color: var(--muted); white-space: normal; overflow-wrap: anywhere; font-size: 11px; }
</style>
