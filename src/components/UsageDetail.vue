<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { ChevronLeft, ChevronRight, ExternalLink, FileText } from 'lucide-vue-next';
import UsageDrawer from './UsageDrawer.vue';
import { cacheRate, compact, dateTime, modelLabel, money, number, projectLabel, referenceCost, roleLabel, totalTokens, usageDetail, usageOpenSource } from '../usage';
import type { UsageCostGroup, UsageDetail as Detail, UsageFilters, UsageRecord } from '../usage';

const props = defineProps<{ threadId: string; filters: UsageFilters }>();
const emit = defineEmits<{ close: []; openMemory: [id: string] }>();
const data = ref<Detail>();
const thread = ref(props.threadId);
const whole = ref(false);
const page = ref(0);
const tab = ref<'records' | 'source'>('records');
const selected = ref<UsageRecord>();
const loading = ref(false);
const error = ref('');
let version = 0;
const summary = computed(() => data.value?.session.summary);
const breakdown = computed(() => summary.value ? [
  { label: summary.value.cacheKnownInput < summary.value.input ? '其他输入' : '新增输入', value: summary.value.input - summary.value.cached - summary.value.cacheWrite, color: 'var(--accent)' },
  { label: '缓存输入', value: summary.value.cached, color: 'var(--usage-cache, #92acfb)' },
  { label: '缓存写入', value: summary.value.cacheWrite, color: 'var(--repo-b)' },
  { label: '输出', value: summary.value.output, color: 'var(--usage-output, #67717e)' },
] .filter(item => item.label !== '缓存写入' || item.value) : []);

async function load() {
  const id = ++version;
  loading.value = true; error.value = '';
  try {
    const filters = whole.value ? { ...props.filters, start: 0, end: Date.now() + 1, project: null, model: null, role: null, buckets: [] } : props.filters;
    const result = await usageDetail(filters, thread.value, page.value);
    if (id !== version) return;
    data.value = result; selected.value = result.records[0];
  } catch (cause) { if (id === version) error.value = String(cause); }
  finally { if (id === version) loading.value = false; }
}
async function openSource() {
  if (!selected.value) return;
  try { await usageOpenSource(selected.value.id); } catch (cause) { error.value = String(cause); }
}
function parent() { thread.value = data.value!.session.parentId!; whole.value = true; page.value = 0; }
function costLines(group: UsageCostGroup) {
  return [
    { label: '新增输入', tokens: group.input - group.cached - group.cacheWrite, price: group.price.input },
    { label: '缓存读取', tokens: group.cached, price: group.price.cached },
    { label: '缓存写入', tokens: group.cacheWrite, price: group.price.cacheWrite },
    { label: '输出', tokens: group.output, price: group.price.output },
  ].filter(line => line.label !== '缓存写入' || line.tokens);
}
watch([thread, whole, page, () => props.filters], load, { immediate: true });
onBeforeUnmount(() => { version++; });
</script>

<template>
  <UsageDrawer title="会话详情" @close="emit('close')">
    <p v-if="error" class="usage-error" role="alert">{{ error }}</p>
    <p v-if="loading && !data" class="usage-muted">正在读取会话用量…</p>
    <template v-if="data && summary">
      <h3 class="detail-title">{{ data.session.title }}</h3>
      <p class="detail-subtitle">{{ projectLabel(data.session.project) }} · {{ roleLabel(data.session.role) }}</p>
      <div class="detail-scope"><span>{{ whole ? '完整会话' : '当前筛选范围' }}</span><button v-if="!whole" class="text-button" @click="whole = true; page = 0">查看完整会话</button></div>
      <dl class="detail-metadata">
        <div><dt>模型</dt><dd>{{ modelLabel(data.session.model) }}</dd></div>
        <div><dt>用量记录</dt><dd>{{ number(summary.records) }} 条</dd></div>
        <div><dt>活动时间</dt><dd>{{ dateTime(data.session.firstAt) }}—{{ dateTime(data.session.lastAt) }}</dd></div>
        <div><dt>来源</dt><dd>Codex 本地日志</dd></div>
      </dl>
      <button v-if="data.session.parentId" class="text-button parent-link" @click="parent">查看父会话 <ExternalLink :size="12" /></button>

      <section class="detail-section">
        <h4>Token 明细</h4>
        <strong class="detail-big" :title="number(totalTokens(summary))">{{ compact(totalTokens(summary)) }}</strong><p class="usage-muted">已记录 Tokens · 缓存命中 {{ cacheRate(summary) }}</p>
        <div v-for="item in breakdown" :key="item.label" class="detail-token-row">
          <span>{{ item.label }}</span><div class="detail-track"><i :style="{ width: `${totalTokens(summary) ? item.value / totalTokens(summary) * 100 : 0}%`, background: item.color }" /></div><strong>{{ number(item.value) }}</strong>
        </div>
        <div class="detail-reasoning"><span>其中推理</span><span>{{ summary.reasoningKnownOutput < summary.output ? summary.reasoningKnownOutput ? `已知 ${number(summary.reasoning)}` : '未返回' : number(summary.reasoning) }}</span></div>
        <p class="usage-note">输入已包含缓存，输出已包含推理，均不重复相加。<template v-if="summary.cacheKnownInput < summary.input">部分记录未返回缓存分项。</template></p>
      </section>

      <section class="detail-section">
        <h4>{{ summary.pricedRecords && summary.pricedRecords < summary.records ? '已知参考费用' : '参考费用' }}</h4>
        <strong class="detail-big cost">{{ referenceCost(summary) }}</strong>
        <template v-if="summary.pricedRecords">
          <div v-for="(label, index) in ['新增输入', '缓存读取', '缓存写入', '输出']" :key="label" class="detail-cost-row"><span>{{ label }}</span><strong>{{ money(data.costParts[index], 6) }}</strong></div>
          <details class="detail-price-calculation"><summary>查看单价与计算</summary><div v-for="group in data.costGroups" :key="`${group.price.model}/${group.price.provider}/${group.price.tier}`" class="detail-price-group"><strong>{{ group.price.model }} · {{ group.price.tier || '默认参考档位' }}</strong><p class="usage-note">{{ group.price.provider || '未记录供应商' }} · {{ group.price.source }} · 美元 / 百万 Tokens</p><div v-for="line in costLines(group)" :key="line.label" class="detail-price-line"><span>{{ line.label }} {{ number(line.tokens) }} × {{ money(line.price) }} / 1M</span><strong>{{ money(line.price === null ? null : line.tokens * line.price / 1e6, 6) }}</strong></div></div></details>
        </template>
        <p class="usage-note">按配置价格估算 · {{ number(summary.pricedRecords) }} / {{ number(summary.records) }} 条已定价。参考费用不代表订阅扣费。</p>
      </section>

      <section class="detail-section">
        <div class="usage-tabs detail-tabs" role="tablist" aria-label="会话详情视图"><button :class="{ active: tab === 'records' }" role="tab" :aria-selected="tab === 'records'" @click="tab = 'records'">用量记录</button><button :class="{ active: tab === 'source' }" role="tab" :aria-selected="tab === 'source'" @click="tab = 'source'">来源信息</button></div>
        <div v-if="tab === 'records'" class="usage-table-scroll">
          <table class="usage-table detail-table"><thead><tr><th>时间</th><th>模型</th><th class="numeric">Tokens</th><th class="numeric">参考费用</th></tr></thead><tbody><tr v-for="record in data.records" :key="record.id" :class="{ selected: selected?.id === record.id }"><td><button class="usage-row-button" @click="selected = record; tab = 'source'">{{ new Date(record.at).toLocaleTimeString('zh-CN', { hour12: false }) }}</button></td><td :title="record.model">{{ modelLabel(record.model) }}</td><td class="numeric">{{ number(record.tokens.input + record.tokens.output) }}</td><td class="numeric">{{ money(record.cost) }}</td></tr></tbody></table>
          <div v-if="data.totalRecords > 100" class="usage-pagination"><button class="icon-button" :disabled="page === 0 || loading" aria-label="上一页记录" @click="page--"><ChevronLeft :size="16" /></button><span>{{ page + 1 }} / {{ Math.ceil(data.totalRecords / 100) }}</span><button class="icon-button" :disabled="(page + 1) * 100 >= data.totalRecords || loading" aria-label="下一页记录" @click="page++"><ChevronRight :size="16" /></button></div>
        </div>
        <dl v-else-if="selected" class="detail-source">
          <div><dt>日志文件</dt><dd>{{ selected.file }}</dd></div><div><dt>记录位置</dt><dd>第 {{ selected.line }} 行</dd></div><div><dt>用量依据</dt><dd>{{ { response: '响应级用量', last: '日志单次用量', cumulative: '累计新增用量' }[selected.basis] ?? selected.basis }}</dd></div><div><dt>供应商标识</dt><dd>{{ selected.provider || '未记录' }}</dd></div><div><dt>服务档位</dt><dd>{{ selected.tier || '未记录，参考价格使用默认档位' }}</dd></div><div><dt>工作目录</dt><dd>{{ selected.project }}</dd></div><div><dt>线程 / 轮次</dt><dd>{{ selected.threadId }} / {{ selected.turnId || '未记录' }}</dd></div>
        </dl>
      </section>
      <section v-if="data.memories.length" class="detail-section"><h4>关联归档事项</h4><button v-for="memory in data.memories" :key="memory.id" class="detail-memory text-button" @click="emit('openMemory', memory.id)"><FileText :size="14" />{{ memory.title }}</button></section>
    </template>
    <template #footer><span class="usage-note">仅统计本机已记录用量</span><button class="button" :disabled="!selected" @click="openSource"><ExternalLink :size="14" />打开本地日志</button></template>
  </UsageDrawer>
</template>

<style scoped>
.detail-title { font-size: 21px; line-height: 1.5; margin: 0 0 5px; overflow-wrap: anywhere; }
.detail-subtitle { color: var(--muted); font-size: 13px; margin: 0; }
.detail-scope { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin: 18px 0; font-size: 12px; color: var(--muted); }
.detail-scope > span { border: 1px solid var(--line); padding: 3px 7px; border-radius: 4px; }
.detail-metadata { display: grid; grid-template-columns: 1fr 1fr; gap: 14px; margin: 0; font-size: 12px; }
.detail-metadata > div { min-width: 0; }.detail-metadata > div:nth-child(3) { grid-column: 1 / -1; }
dt { color: var(--muted); margin-bottom: 4px; } dd { margin: 0; overflow-wrap: anywhere; }
.parent-link { display: flex; align-items: center; gap: 5px; margin-top: 14px; }
.detail-section { margin-top: 24px; padding-top: 22px; border-top: 1px solid var(--line); }
h4 { font-size: 15px; margin: 0 0 10px; }.detail-big { display: block; font-size: 32px; font-weight: 650; line-height: 1.3; letter-spacing: -.6px; font-variant-numeric: tabular-nums; }
.detail-big.cost { margin-bottom: 16px; }.usage-muted { font-size: 12px; color: var(--muted); margin: 5px 0 18px; }
.detail-token-row { display: grid; grid-template-columns: 76px 1fr 94px; align-items: center; gap: 12px; margin-top: 12px; font-size: 12px; }
.detail-token-row strong { text-align: right; font-weight: 500; font-variant-numeric: tabular-nums; }
.detail-track { height: 8px; background: var(--surface); border-radius: 2px; overflow: hidden; }.detail-track i { display: block; height: 100%; border-radius: inherit; }
.detail-reasoning { display: flex; justify-content: space-between; padding-left: 12px; margin-top: 10px; color: var(--muted); font-size: 12px; }
.detail-cost-row { display: flex; justify-content: space-between; font-size: 13px; padding: 7px 0; border-bottom: 1px solid var(--line); }.detail-cost-row strong { font-weight: 500; }
.detail-price-calculation { margin-top: 14px; font-size: 12px; }.detail-price-calculation > summary { cursor: pointer; color: var(--accent-text); }.detail-price-group { margin-top: 16px; }.detail-price-group > strong { font-size: 12px; font-weight: 500; }.detail-price-line { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; margin-top: 9px; font-size: 11px; }.detail-price-line > strong { font-weight: 500; }
.usage-note { font-size: 11px; line-height: 1.7; color: var(--muted); margin: 12px 0 0; }
.detail-tabs { margin: -10px 0 12px; }.detail-table { min-width: 430px; font-size: 11px; }.detail-table th, .detail-table td { padding: 10px 8px; }
.detail-source { font-size: 12px; margin: 18px 0 0; }.detail-source > div + div { margin-top: 16px; }
.detail-memory { display: flex; align-items: center; gap: 8px; margin-top: 12px; text-align: left; }
@media (max-width: 500px) { .detail-token-row { grid-template-columns: 64px 1fr 82px; gap: 8px; } }
</style>
