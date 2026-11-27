<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { compact, number, referenceCost, totalTokens } from '../usage';
import type { UsageOverview, UsageSummary } from '../usage';

const props = defineProps<{ trend: UsageOverview['trend']; summary: UsageSummary; rangeText: string; hourly: boolean }>();
const metric = ref<'tokens' | 'records' | 'cost'>('tokens');
const hovered = ref<number | null>(null);
const chart = ref<HTMLDivElement>();
const width = ref(700);
let observer: ResizeObserver;
const selected = computed(() => hovered.value === null ? undefined : props.trend[hovered.value]);
const plotWidth = computed(() => width.value - 65);
const labelStep = computed(() => Math.max(1, Math.ceil(props.trend.length / Math.max(2, Math.floor(plotWidth.value / 50)))));
const max = computed(() => {
  const largest = Math.max(0, ...props.trend.map(row => value(row.summary)));
  if (!largest) return 1;
  const unit = 10 ** Math.floor(Math.log10(largest));
  return Math.ceil(largest / unit / 2) * unit * 2;
});

function value(s: UsageSummary) { return metric.value === 'tokens' ? totalTokens(s) : metric.value === 'records' ? s.records : s.cost; }
function stack(s: UsageSummary) {
  if (metric.value !== 'tokens') return [{ value: value(s), color: 'var(--accent)' }];
  return [
    { value: s.input - s.cached - s.cacheWrite, color: 'var(--accent)' },
    { value: s.cached, color: 'var(--usage-cache)' },
    { value: s.cacheWrite, color: 'var(--repo-b)' },
    { value: s.output, color: 'var(--usage-output)' },
  ];
}
function barY(s: UsageSummary, index: number) { return 165 - stack(s).slice(0, index + 1).reduce((sum, part) => sum + part.value, 0) / max.value * 140; }
onMounted(() => { observer = new ResizeObserver(([entry]) => { width.value = entry.contentRect.width; }); observer.observe(chart.value!); });
onBeforeUnmount(() => observer.disconnect());
</script>

<template>
  <section class="usage-chart-card glass-surface">
    <header><h2>{{ hourly ? '每小时用量' : '每日用量' }}</h2><div class="usage-segments small"><button v-for="option in [{ value: 'tokens' as const, label: 'Tokens' }, { value: 'records' as const, label: '记录' }, { value: 'cost' as const, label: '费用' }]" :key="option.value" :class="{ active: metric === option.value }" :aria-pressed="metric === option.value" @click="metric = option.value">{{ option.label }}</button></div></header>
    <div v-if="metric === 'tokens'" class="usage-legend"><span><i class="fresh" />{{ summary.cacheKnownInput < summary.input ? '其他输入' : '新增输入' }}</span><span><i class="cached" />缓存输入</span><span v-if="summary.cacheWrite"><i class="write" />缓存写入</span><span><i class="output" />输出</span></div>
    <p v-if="metric === 'cost' && summary.pricedRecords < summary.records" class="usage-note">仅显示已定价记录的参考费用</p>
    <div ref="chart" class="usage-chart" @pointerleave="hovered = null">
      <svg :viewBox="`0 0 ${width} 200`" role="img" :aria-label="`${rangeText} ${metric === 'tokens' ? 'Token' : metric === 'cost' ? '参考费用' : '用量记录'}趋势`">
        <g v-for="tick in [0,1,2,3,4]" :key="tick"><line x1="55" :x2="width - 8" :y1="165 - tick * 35" :y2="165 - tick * 35" stroke="var(--line)" stroke-width=".7" /><text x="44" :y="170 - tick * 35" text-anchor="end" fill="var(--muted)" font-size="11">{{ metric === 'cost' ? `$${(max * tick / 4).toFixed(2)}` : compact(max * tick / 4) }}</text></g>
        <g v-for="(row, index) in trend" :key="row.start" tabindex="0" @pointerenter="hovered = index" @focus="hovered = index" @blur="hovered = null">
          <title>{{ row.label }}：{{ number(totalTokens(row.summary)) }} Tokens，{{ row.summary.records }} 条记录，{{ referenceCost(row.summary) }}</title>
          <rect :x="57 + index * plotWidth / trend.length" y="20" :width="plotWidth / trend.length" height="149" fill="transparent" />
          <rect v-for="(part, n) in stack(row.summary)" :key="n" :x="57 + (index + .21) * plotWidth / trend.length" :y="barY(row.summary, n)" :width="Math.max(1, plotWidth / trend.length * .58)" :height="part.value / max * 140" :fill="part.color" :opacity="hovered === null || hovered === index ? 1 : .5" />
          <text v-if="index % labelStep === 0" :x="57 + (index + .5) * plotWidth / trend.length" y="190" text-anchor="middle" fill="var(--muted)" font-size="10">{{ row.label }}</text>
        </g>
      </svg>
      <div v-if="selected" class="usage-chart-tooltip"><strong>{{ selected.label }}</strong><span>{{ compact(totalTokens(selected.summary)) }} Tokens</span><span>输入 {{ compact(selected.summary.input) }} · 缓存 {{ compact(selected.summary.cached) }} · 输出 {{ compact(selected.summary.output) }}</span><span>{{ selected.summary.records }} 条记录 · {{ referenceCost(selected.summary) }}</span></div>
      <p v-if="!summary.records" class="usage-chart-empty">当前范围没有用量记录</p>
    </div>
  </section>
</template>
