<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { CalendarDays, ChevronLeft, ChevronRight, Clock3, Database, Download, ExternalLink, RefreshCw, SlidersHorizontal } from 'lucide-vue-next';
import { openUrl } from '@tauri-apps/plugin-opener';
import AppSelect from './AppSelect.vue';
import UsageDrawer from './UsageDrawer.vue';
import UsageDetail from './UsageDetail.vue';
import UsageTrend from './UsageTrend.vue';
import { cacheRate, compact, dateInput, dateTime, modelLabel, number, projectLabel, referenceCost, roleLabel, time, timeBuckets, totalTokens, usageExport, usageIssues, usageOverview, usagePreferencesSave, usagePricesGet, usagePricesSave, usageScan, usageSessions } from '../usage';
import type { UsageFilters, UsageGroup, UsageIssue, UsageOverview, UsagePrice, UsageSession, UsageSummary } from '../usage';

const emit = defineEmits<{ settings: []; openMemory: [id: string] }>();
const data = ref<UsageOverview>();
const sessions = ref<UsageSession[]>([]);
const sessionTotal = ref(0);
const range = ref('7');
const project = ref(''); const model = ref(''); const role = ref('');
const unidentified = '__unidentified__';
const from = ref(dateInput(new Date(Date.now() - 6 * 86400000))); const to = ref(dateInput(new Date()));
const view = ref<'sessions' | 'models' | 'projects'>('sessions');
const sort = ref('recent'); const page = ref(0);
const loading = ref(false); const syncing = ref(false); const exporting = ref(false);
const error = ref(''); const message = ref('');
const panel = ref<'sources' | 'prices' | 'issues' | null>(null);
const issues = ref<UsageIssue[]>([]);
const selectedThread = ref<string | null>(null);
const showCalendar = ref(false); const calendarRange = ref('12'); const calendar = ref<UsageOverview>();
const saving = ref(false);
type PriceDraft = Omit<UsagePrice, 'input' | 'cached' | 'cacheWrite' | 'output'> & Record<'input' | 'cached' | 'cacheWrite' | 'output', string | number>;
const prices = ref<PriceDraft[]>([]);
const priceKeys = ['input', 'cached', 'cacheWrite', 'output'] as const;
const priceLabels = { input: '输入', cached: '缓存读取', cacheWrite: '缓存写入', output: '输出' };
let requestVersion = 0;
let panelVersion = 0;
let timer: ReturnType<typeof setInterval>;
let lastSyncRequest = 0;
const appliedFilters = ref<UsageFilters>(filters());

const summary = computed(() => data.value?.summary);
const projectOptions = computed(() => [{ value: '', label: '全部项目' }, ...(data.value?.availableProjects ?? []).map(value => ({ value: value || unidentified, label: projectLabel(value) }))]);
const modelOptions = computed(() => [{ value: '', label: '全部模型' }, ...(data.value?.availableModels ?? []).map(value => ({ value: value || unidentified, label: modelLabel(value) }))]);
const roleOptions = computed(() => [{ value: '', label: '全部角色' }, ...(data.value?.roles ?? []).map(value => ({ value, label: roleLabel(value) }))]);
const rangeText = computed(() => `${dateTime(appliedFilters.value.start).split(' ')[0]}—${dateTime(appliedFilters.value.end - 1).split(' ')[0]}`);
const pricedCoverage = computed(() => summary.value && totalTokens(summary.value) ? `${(summary.value.pricedTokens / totalTokens(summary.value) * 100).toFixed(1)}%` : '—');
const quotaWindows = computed(() => [data.value?.quota?.data.primary, data.value?.quota?.data.secondary].filter(window => !!window));
const calendarMax = computed(() => Math.max(1, ...(calendar.value?.trend ?? []).map(row => totalTokens(row.summary))));
const calendarGrid = computed(() => {
  const rows = calendar.value?.trend ?? [];
  if (!rows.length) return { days: [], months: [], weeks: 12 };
  const start = new Date(rows[0].start);
  start.setDate(start.getDate() - (start.getDay() + 6) % 7);
  const end = new Date(rows.at(-1)!.start);
  end.setDate(end.getDate() + 6 - (end.getDay() + 6) % 7);
  const byDate = new Map(rows.map(row => [dateInput(new Date(row.start)), row]));
  const days = [];
  const months: Array<{ label: string; column: number; span: number }> = [];
  for (const cursor = new Date(start); cursor <= end; cursor.setDate(cursor.getDate() + 1)) {
    const date = dateInput(cursor);
    const row = byDate.get(date);
    const tokens = row ? totalTokens(row.summary) : 0;
    const column = Math.floor(days.length / 7) + 1;
    if (days.length % 7 === 0) {
      const middle = new Date(cursor); middle.setDate(middle.getDate() + 3);
      const label = `${middle.getMonth() + 1} 月`;
      if (months.at(-1)?.label === label) months.at(-1)!.span++;
      else months.push({ label, column, span: 1 });
    }
    days.push({ date, day: cursor.getDate(), tokens, inRange: !!row, level: tokens ? Math.max(1, Math.ceil(tokens / calendarMax.value * 4)) : 0 });
  }
  return { days, months, weeks: days.length / 7 };
});

function filters(): UsageFilters {
  const now = new Date();
  let start: Date; let end: Date;
  if (range.value === 'custom') {
    start = new Date(`${from.value}T00:00:00`); end = new Date(`${to.value}T00:00:00`); end.setDate(end.getDate() + 1);
  } else {
    start = new Date(now); start.setHours(0, 0, 0, 0); start.setDate(start.getDate() - (Number(range.value) - 1)); end = now;
  }
  const a = start.getTime(); const b = end.getTime();
  return { start: a, end: b, project: project.value === unidentified ? '' : project.value || null, model: model.value === unidentified ? '' : model.value || null, role: role.value || null, buckets: Number.isFinite(a) && Number.isFinite(b) && a < b ? timeBuckets(a, b, range.value === '1') : [] };
}
async function load() {
  const id = ++requestVersion;
  const f = filters();
  if (!Number.isFinite(f.start) || !Number.isFinite(f.end) || f.start >= f.end) { error.value = '请选择有效的起止日期'; return; }
  loading.value = true;
  try {
    const [overview, rows] = await Promise.all([usageOverview(f), usageSessions(f, page.value, sort.value)]);
    if (id !== requestVersion) return;
    data.value = overview; sessions.value = rows.items; sessionTotal.value = rows.total; appliedFilters.value = f; error.value = '';
  } catch (cause) { if (id === requestVersion) error.value = String(cause); }
  finally { if (id === requestVersion) loading.value = false; }
}
async function sync(full = false) {
  if (syncing.value || data.value?.status.scanning) return;
  syncing.value = true; message.value = ''; lastSyncRequest = Date.now();
  try { await usageScan(full); await load(); if (showCalendar.value) await loadCalendar(); }
  catch (cause) { error.value = String(cause); }
  finally { syncing.value = false; }
}
async function exportData() {
  exporting.value = true;
  try { if (await usageExport(appliedFilters.value)) message.value = '已导出当前筛选范围的全部用量记录'; }
  catch (cause) { error.value = String(cause); }
  finally { exporting.value = false; }
}
async function openPanel(next: 'sources' | 'prices' | 'issues') {
  const id = ++panelVersion;
  panel.value = next; error.value = '';
  try {
    if (next === 'prices') {
      const rows = await usagePricesGet();
      if (id !== panelVersion) return;
      prices.value = rows.map(p => ({ ...p, input: p.input?.toString() ?? '', cached: p.cached?.toString() ?? '', cacheWrite: p.cacheWrite?.toString() ?? '', output: p.output?.toString() ?? '' }));
    } else if (next === 'issues') { const rows = await usageIssues(); if (id === panelVersion) issues.value = rows; }
  } catch (cause) { if (id === panelVersion) error.value = String(cause); }
}
async function savePrices() {
  saving.value = true;
  try {
    const price = (value: string | number) => String(value).trim() === '' ? null : Number(value);
    const rows: UsagePrice[] = prices.value.map(p => ({ ...p, input: price(p.input), cached: price(p.cached), cacheWrite: price(p.cacheWrite), output: price(p.output) }));
    if (rows.some(p => priceKeys.some(key => p[key] !== null && (!Number.isFinite(p[key]) || p[key]! < 0)))) throw new Error('价格请输入非负数，留空表示未定价');
    await usagePricesSave(rows); panel.value = null; message.value = '参考价格已保存'; await load();
    if (showCalendar.value) await loadCalendar();
  } catch (cause) { error.value = String(cause); }
  finally { saving.value = false; }
}
async function restorePrices() {
  saving.value = true;
  try {
    await usagePricesSave(prices.value.map(p => ({ ...p, input: null, cached: null, cacheWrite: null, output: null, custom: false })));
    await openPanel('prices'); await load(); message.value = '已恢复 OpenAI 官方默认定价';
  } catch (cause) { error.value = String(cause); }
  finally { saving.value = false; }
}
async function savePreference(autoScan: boolean, intervalSeconds: number) {
  try { const preferences = await usagePreferencesSave({ autoScan, intervalSeconds }); if (data.value) data.value.preferences = preferences; }
  catch (cause) { error.value = String(cause); }
}
async function loadCalendar() {
  try {
    const start = new Date(); start.setHours(0, 0, 0, 0);
    if (calendarRange.value === 'year') { start.setFullYear(start.getFullYear(), 0, 1); }
    else { start.setDate(start.getDate() - (start.getDay() + 6) % 7 - 77); }
    const f = filters(); f.start = start.getTime(); f.end = Date.now(); f.buckets = timeBuckets(f.start, f.end);
    calendar.value = await usageOverview(f);
  } catch (cause) { error.value = String(cause); }
}
function quotaName(minutes: number | null | undefined) { if (!minutes) return '额度'; if (minutes === 10080) return '周额度'; return minutes % 60 === 0 ? `${minutes / 60} 小时额度` : `${minutes} 分钟额度`; }
function applyGroup(group: UsageGroup) { if (view.value === 'models') model.value = group.key || unidentified; else project.value = group.key || unidentified; view.value = 'sessions'; }
function tick() {
  if (document.hidden || loading.value || syncing.value || !data.value) return;
  if (data.value.status.scanning) { void load(); return; }
  if (data.value.preferences.autoScan && Date.now() - Math.max(data.value.status.lastSyncedAt ?? 0, lastSyncRequest) >= data.value.preferences.intervalSeconds * 1000) void sync();
}
function visibility() { if (!document.hidden) { void load(); } }
watch([range, project, model, role, from, to], () => { page.value = 0; selectedThread.value = null; void load(); if (showCalendar.value) void loadCalendar(); });
watch([page, sort], load);
watch([showCalendar, calendarRange], () => { if (showCalendar.value) void loadCalendar(); });
onMounted(() => { timer = setInterval(tick, 2000); document.addEventListener('visibilitychange', visibility); void load().then(() => { if (data.value?.preferences.autoScan) void sync(); }); });
onBeforeUnmount(() => { requestVersion++; panelVersion++; clearInterval(timer); document.removeEventListener('visibilitychange', visibility); });
</script>

<template>
  <section class="codex-usage" data-testid="codex-usage" :aria-busy="loading">
    <div class="usage-toolbar"><p class="usage-source"><Database :size="13" />Codex 本地日志</p><div class="usage-actions"><button class="button" @click="openPanel('sources')"><SlidersHorizontal :size="14" />数据来源</button><button class="button" @click="openPanel('prices')">定价</button><button class="button primary" :disabled="syncing || data?.status.scanning" @click="sync()"><RefreshCw :size="14" :class="{ spinning: syncing || data?.status.scanning }" />{{ syncing || data?.status.scanning ? '正在同步' : '立即同步' }}</button></div></div>
    <div class="usage-filter-row"><div class="usage-segments" aria-label="统计时间范围"><button v-for="option in [{ value: '1', label: '今日' }, { value: '7', label: '近 7 天' }, { value: '30', label: '近 30 天' }, { value: 'custom', label: '自定义' }]" :key="option.value" :class="{ active: range === option.value }" :aria-pressed="range === option.value" @click="range = option.value">{{ option.label }}</button></div><div class="usage-selects"><AppSelect v-model="project" :options="projectOptions" aria-label="按项目筛选" data-testid="usage-project" /><AppSelect v-model="model" :options="modelOptions" aria-label="按模型筛选" data-testid="usage-model" /><AppSelect v-model="role" :options="roleOptions" aria-label="按角色筛选" data-testid="usage-role" /></div></div>
    <div v-if="range === 'custom'" class="usage-custom-range"><label>开始日期<input v-model="from" type="date" /></label><span>至</span><label>结束日期<input v-model="to" type="date" /></label></div>
    <div class="usage-sync-line"><span>{{ rangeText }}</span><span v-if="data?.status.scanning">已处理 {{ data.status.completed }} / {{ data.status.total }} 个文件</span><span v-else-if="data">{{ data.status.lastSyncedAt ? `${time(data.status.lastSyncedAt)} 已同步` : '尚未同步' }} · {{ data.preferences.autoScan ? `每 ${data.preferences.intervalSeconds} 秒` : '手动同步' }}</span></div>
    <p v-if="error && !panel" class="usage-error" role="alert">{{ error }}</p><p v-if="message" class="usage-message" role="status">{{ message }}</p>
    <div v-if="data?.status.errors.length" class="usage-sync-errors"><details><summary>{{ data.status.errors.length }} 个来源同步失败</summary><p v-for="item in data.status.errors" :key="item">{{ item }}</p></details></div>
    <button v-if="data?.issueCount" class="usage-issue-link" @click="openPanel('issues')">{{ number(data.issueCount) }} 条记录待确认，查看来源 <ChevronRight :size="13" /></button>
    <template v-if="summary && data">
      <div class="usage-metrics" data-testid="usage-metrics"><div><span>已记录 Tokens</span><strong :title="number(totalTokens(summary))">{{ compact(totalTokens(summary)) }}</strong><small>输入 {{ compact(summary.input) }} · 输出 {{ compact(summary.output) }}</small></div><div><span>缓存命中率</span><strong>{{ cacheRate(summary) }}</strong><small>缓存输入 {{ compact(summary.cached) }}{{ summary.cacheKnownInput < summary.input ? ' · 部分记录' : '' }}</small></div><div><span title="确认新增用量后生成的记录数，不代表 HTTP 请求数">用量记录</span><strong>{{ number(summary.records) }}</strong><small>{{ number(summary.sessions) }} 个会话</small></div><div><span>{{ summary.pricedRecords && summary.pricedRecords < summary.records ? '已知参考费用' : '参考费用' }}</span><strong :class="{ unpriced: !summary.pricedRecords }">{{ referenceCost(summary) }}</strong><small>{{ summary.pricedRecords ? `已定价 ${pricedCoverage}${summary.pricedRecords < summary.records ? ` · ${number(summary.records - summary.pricedRecords)} 条未定价` : ''}` : summary.records ? '暂无匹配价格，可手动补充' : '尚无用量记录' }}</small></div></div>
      <div class="usage-quota"><span class="usage-quota-title" :title="data.quota ? `快照 ${dateTime(data.quota.at)}，不受日期筛选影响` : ''"><Clock3 :size="14" />额度快照</span><template v-if="quotaWindows.length"><div v-for="(window, index) in quotaWindows" :key="index" class="usage-quota-window"><span>{{ quotaName(window.window_minutes) }} · 已用 {{ window.used_percent }}%</span><div class="usage-quota-track"><i :style="{ width: `${Math.min(100, Math.max(0, window.used_percent))}%` }" /></div><small v-if="window.resets_at">{{ window.resets_at * 1000 <= Date.now() ? '等待新快照' : `${dateTime(window.resets_at * 1000)} 重置` }}</small></div><small class="usage-quota-time">{{ time(data.quota!.at) }} 快照</small></template><span v-else class="usage-muted">暂无额度快照</span><button class="text-button usage-official" @click="openUrl('https://chatgpt.com/codex/settings/usage').catch(cause => error = String(cause))">官方用量 <ExternalLink :size="12" /></button></div>

      <div class="usage-chart-grid"><UsageTrend :trend="data.trend" :summary="summary" :range-text="rangeText" :hourly="range === '1'" /><section class="usage-model-card"><header><h2>模型分布</h2><span>{{ compact(totalTokens(summary)) }} Tokens</span></header><div v-if="!data.models.length" class="usage-model-empty">暂无模型用量</div><button v-for="group in data.models.slice(0,5)" :key="group.key" class="usage-model-row" :title="modelLabel(group.key)" @click="model = group.key || unidentified"><span>{{ modelLabel(group.key) }}</span><div><i :style="{ width: `${totalTokens(summary) ? totalTokens(group.summary) / totalTokens(summary) * 100 : 0}%` }" /></div><strong>{{ compact(totalTokens(group.summary)) }}</strong><small>{{ totalTokens(summary) ? (totalTokens(group.summary) / totalTokens(summary) * 100).toFixed(0) : 0 }}%</small></button></section></div>

      <section v-if="showCalendar" class="usage-calendar-card" :class="{ 'usage-calendar-year': calendarRange === 'year' }" :style="{ '--calendar-weeks': calendarGrid.weeks }">
        <header><h2>使用日历</h2><AppSelect v-model="calendarRange" :options="[{ value: '12', label: '最近 12 周' }, { value: 'year', label: '今年' }]" aria-label="日历范围" /></header>
        <div class="usage-calendar-layout">
          <div class="usage-calendar-weekdays" aria-hidden="true"><span v-for="day in ['一', '二', '三', '四', '五', '六', '日']" :key="day">{{ day }}</span></div>
          <div class="usage-calendar-scroll">
            <div class="usage-calendar-months" aria-hidden="true"><span v-for="month in calendarGrid.months" :key="month.column" :style="{ gridColumn: `${month.column} / span ${month.span}` }">{{ month.label }}</span></div>
            <div class="usage-heatmap" aria-label="每日 Token 用量，周一至周日从上到下排列">
              <button v-for="day in calendarGrid.days" :key="day.date" :disabled="!day.inRange" :data-level="day.level" :aria-label="`${day.date}：${number(day.tokens)} Tokens`" :title="`${day.date} · ${number(day.tokens)} Tokens`" :aria-pressed="range === 'custom' && from === day.date && to === day.date" @click="range = 'custom'; from = day.date; to = day.date"><span>{{ day.day }}</span></button>
            </div>
          </div>
        </div>
        <footer><p class="usage-note">点击日期查看当天用量 · 沿用项目、模型和角色筛选</p><div class="usage-calendar-legend" aria-label="颜色越深用量越高"><span>少</span><i v-for="level in [0,1,2,3,4]" :key="level" :data-level="level" /><span>多</span></div></footer>
      </section>

      <section class="usage-log"><header><div class="usage-tabs" role="tablist" aria-label="用量明细"><button role="tab" :aria-selected="view === 'sessions'" :class="{ active: view === 'sessions' }" @click="view = 'sessions'">会话明细</button><button role="tab" :aria-selected="view === 'models'" :class="{ active: view === 'models' }" @click="view = 'models'">模型汇总</button><button role="tab" :aria-selected="view === 'projects'" :class="{ active: view === 'projects' }" @click="view = 'projects'">项目汇总</button></div><div class="usage-actions"><button class="button" :aria-pressed="showCalendar" @click="showCalendar = !showCalendar"><CalendarDays :size="14" />使用日历</button><button class="button" :disabled="exporting || !summary.records" @click="exportData"><Download :size="14" />{{ exporting ? '正在导出' : '导出' }}</button></div></header>
        <div v-if="view === 'sessions'" class="usage-table-scroll"><table class="usage-table"><thead><tr><th><button @click="sort = 'recent'">最近活动</button></th><th>会话 / 项目</th><th>模型</th><th>角色</th><th class="numeric"><button @click="sort = 'tokens'">Tokens</button></th><th class="numeric">缓存命中</th><th class="numeric"><button @click="sort = 'cost'">参考费用</button></th><th /></tr></thead><tbody><tr v-for="session in sessions" :key="session.id" :data-usage-thread="session.id"><td>{{ dateTime(session.lastAt) }}</td><td class="usage-session-cell"><button class="usage-row-button" :title="session.title" @click="selectedThread = session.id">{{ session.title }}</button><small :title="session.project">{{ projectLabel(session.project) }}</small></td><td class="usage-model-cell" :title="session.model">{{ modelLabel(session.model) }}</td><td><span class="usage-role" :class="session.role">{{ roleLabel(session.role) }}</span></td><td class="numeric" :title="number(totalTokens(session.summary))">{{ compact(totalTokens(session.summary)) }}</td><td class="numeric">{{ cacheRate(session.summary) }}</td><td class="numeric" :title="`${session.summary.pricedRecords} / ${session.summary.records} 条已定价`">{{ referenceCost(session.summary) }}{{ session.summary.pricedRecords && session.summary.pricedRecords < session.summary.records ? '*' : '' }}</td><td><button class="icon-button" :aria-label="`查看 ${session.title} 的用量`" @click="selectedThread = session.id"><ChevronRight :size="14" /></button></td></tr></tbody></table><div v-if="!sessions.length" class="usage-empty"><Database :size="24" /><strong>{{ data.status.scanning ? '正在读取本机会话日志' : data.availableModels.length ? '当前筛选没有用量记录' : '暂无 Codex 用量' }}</strong><span>{{ data.status.scanning ? `${data.status.completed} / ${data.status.total} 个文件` : '可以调整时间范围，或检查 Codex 数据目录。' }}</span><button v-if="!data.status.scanning && !data.availableModels.length" class="button" @click="emit('settings')">检查 Codex 目录</button></div></div>
        <div v-else class="usage-table-scroll"><table class="usage-table"><thead><tr><th>{{ view === 'models' ? '模型' : '项目' }}</th><th class="numeric">记录数</th><th class="numeric">会话数</th><th class="numeric">输入</th><th class="numeric">缓存输入</th><th class="numeric">输出</th><th class="numeric">Tokens</th><th class="numeric">参考费用</th></tr></thead><tbody><tr v-for="group in view === 'models' ? data.models : data.projects" :key="group.key"><td><button class="usage-row-button" :title="group.key" @click="applyGroup(group)">{{ view === 'models' ? modelLabel(group.key) : projectLabel(group.key) }}</button></td><td class="numeric">{{ number(group.summary.records) }}</td><td class="numeric">{{ number(group.summary.sessions) }}</td><td class="numeric">{{ compact(group.summary.input) }}</td><td class="numeric">{{ compact(group.summary.cached) }}</td><td class="numeric">{{ compact(group.summary.output) }}</td><td class="numeric">{{ compact(totalTokens(group.summary)) }}</td><td class="numeric">{{ referenceCost(group.summary) }}</td></tr></tbody></table><p v-if="!summary.records" class="usage-note usage-group-empty">当前筛选没有用量记录</p></div>
        <footer><span>仅统计本机已记录用量；参考费用不代表账单。</span><div v-if="view === 'sessions'" class="usage-pagination"><span>共 {{ number(sessionTotal) }} 个会话</span><button class="icon-button" :disabled="page === 0 || loading" aria-label="上一页会话" @click="page--"><ChevronLeft :size="15" /></button><span>{{ page + 1 }} / {{ Math.max(1, Math.ceil(sessionTotal / 20)) }}</span><button class="icon-button" :disabled="(page + 1) * 20 >= sessionTotal || loading" aria-label="下一页会话" @click="page++"><ChevronRight :size="15" /></button></div></footer>
      </section>
    </template><p v-else-if="loading" class="usage-initial-loading">正在读取 Codex 用量…</p>

    <UsageDetail v-if="selectedThread" :thread-id="selectedThread" :filters="appliedFilters" @close="selectedThread = null" @open-memory="emit('openMemory', $event)" />
    <UsageDrawer v-if="panel" :title="panel === 'sources' ? '数据来源' : panel === 'prices' ? '模型参考价格' : '待确认记录'" :wide="panel === 'prices'" @close="panel = null; panelVersion++">
      <p v-if="error" class="usage-error" role="alert">{{ error }}</p>
      <template v-if="panel === 'sources' && data"><h3 class="usage-panel-heading">Codex 会话日志</h3><p class="usage-note">读取 sessions 与 archived_sessions，包含主对话、子代理和系统辅助。同步用量不调用模型。</p><code class="usage-directory">{{ data.directory }}</code><button class="text-button" @click="panel = null; emit('settings')">修改 Codex 目录</button><div class="usage-source-setting"><div><strong>自动扫描会话日志</strong><p>页面可见时按下方间隔同步，其他时间每 10 分钟扫描。</p></div><input type="checkbox" :checked="data.preferences.autoScan" aria-label="自动扫描会话日志" @change="savePreference(($event.target as HTMLInputElement).checked, data.preferences.intervalSeconds)" /></div><label class="usage-interval">页面扫描间隔<AppSelect :model-value="String(data.preferences.intervalSeconds)" :options="[5,10,30,60].map(value => ({ value: String(value), label: `${value} 秒` }))" aria-label="页面扫描间隔" @update:model-value="value => savePreference(data!.preferences.autoScan, Number(value))" /></label><div class="usage-source-setting"><div><strong>重新扫描全部历史</strong><p>重新解析留存日志，按同一规则去重。已删除日志中的历史用量保留。</p></div><button class="button" :disabled="data.status.scanning || syncing" @click="sync(true)">重新扫描</button></div><p class="usage-note">{{ data.status.lastSyncedAt ? `上次同步：${dateTime(data.status.lastSyncedAt)}` : '尚未完成同步' }}</p></template>
      <template v-else-if="panel === 'prices'"><p class="usage-note">默认使用 OpenAI 官方 API 价格，单位为美元 / 百万 Tokens。自动区分普通、Fast 和长上下文；表格展示短上下文单价。手动覆盖后，该行单价适用于全部上下文。</p><button class="text-button usage-pricing-link" @click="openUrl('https://developers.openai.com/api/docs/pricing').catch(cause => error = String(cause))">查看 OpenAI 官方定价 <ExternalLink :size="12" /></button><div class="usage-table-scroll usage-price-table"><table class="usage-table"><thead><tr><th>日志模型 / 供应商 / 档位</th><th>输入</th><th>缓存读取</th><th>缓存写入</th><th>输出</th></tr></thead><tbody><tr v-for="price in prices" :key="`${price.model}/${price.provider}/${price.tier}`"><td><strong>{{ price.model || '未识别模型' }}</strong><small>{{ price.provider || '未记录供应商' }} · {{ price.tier || '默认参考档位' }}</small><small class="usage-price-source">{{ price.custom ? '手动覆盖' : price.source }}</small></td><td v-for="key in priceKeys" :key="key"><input v-model="price[key]" type="number" min="0" step="any" :aria-label="`${price.model} ${price.provider} ${price.tier || '默认档位'} ${priceLabels[key]} 单价`" placeholder="未定价" @input="price.custom = true; price.source = '手动配置'" /></td></tr></tbody></table></div><p class="usage-note">日志未区分缓存写入时长，该项需手动定价；没有公开官方价格的模型可在此补充。</p></template>
      <template v-else-if="panel === 'issues'"><p class="usage-note">以下记录缺少必要依据，未计入已确认用量。先展示最多 500 条。</p><article v-for="issue in issues" :key="`${issue.file}:${issue.line}`" class="usage-issue"><strong>{{ issue.reason }}</strong><code>{{ issue.file }}:{{ issue.line }}</code></article></template>
      <template #footer><template v-if="panel === 'prices'"><button class="button" :disabled="saving || !prices.some(price => price.custom)" @click="restorePrices">恢复官方定价</button><button class="button primary" :disabled="saving || !prices.length" @click="savePrices">{{ saving ? '正在保存' : '保存参考价格' }}</button></template><template v-else><button class="button" @click="panel = null">关闭</button></template></template>
    </UsageDrawer>
  </section>
</template>

<style src="../usage.css"></style>
