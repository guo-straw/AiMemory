<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { ChevronDown, ChevronRight, FileText, Focus, GitBranch, Link2, Maximize2, Minus, Plus, RefreshCw, Search, Settings2, X } from 'lucide-vue-next';
import type { MemoryGraph, MemoryRecord, Repository } from '../types';
import { memoryRead } from '../api';
import { REPOSITORY_COLORS, STAR_MAP_MIN_ZOOM, STAR_MAP_MAX_ZOOM, buildGraphIndex, graphPanelMaxHeight, layoutGraph, scopeGraph } from '../graph';
import type { GraphLabelDensity, GraphRange } from '../graph';
import { StarMapRenderer } from '../graph-renderer';
import { themeTokens } from '../theme';
import MarkdownView from './MarkdownView.vue';
import AppSelect from './AppSelect.vue';

const props = withDefaults(defineProps<{
  graph: MemoryGraph;
  repositories: Repository[];
  initialMemoryId?: string | null;
  active?: boolean;
}>(), { initialMemoryId: null, active: true });
const emit = defineEmits<{ 'open-memory': [id: string] }>();
const canvas = ref<HTMLCanvasElement>();
const topbar = ref<HTMLElement>();
const panelMaxHeight = ref(280);
const searchField = ref<HTMLInputElement>();
const detailPanel = ref<HTMLElement>();
const settingsButton = ref<HTMLButtonElement>();
const selectedId = ref<string | null>(null);
const hoveredId = ref<string | null>(null);
const centerId = ref<string | null>(null);
const range = ref<GraphRange>(0);
const repositoryId = ref<string | null>(null);
const settingsOpen = ref(false);
const showIsolated = ref(true);
const colorByRepository = ref(true);
const showLinks = ref(true);
const labelDensity = ref<GraphLabelDensity>('标准');
const nodeSize = ref(100);
const lineWidth = ref(100);
const zoom = ref(100);
const search = ref('');
const searchOpen = ref(false);
const searchMatchIndex = ref(-1);
const detail = ref<MemoryRecord | null>(null);
const detailLoading = ref(false);
const detailError = ref('');
const dragging = ref(false);
let renderer: StarMapRenderer | undefined;
let resizeObserver: ResizeObserver | undefined;
let detailVersion = 0;
let drag: { pointerId: number; button: number; x: number; y: number; firstX: number; firstY: number; moved: boolean } | null = null;
const themeVersion = ref(0);

const index = computed(() => buildGraphIndex(props.graph));
const nightTokens = computed(() => {
  void themeVersion.value;
  const style = getComputedStyle(document.documentElement);
  return themeTokens({ preset: 'custom', accent: style.getPropertyValue('--accent').trim() || '#3566f0', background: '#080f1d', foreground: '#e4edf9', font: 'system' });
});
const starTheme = computed(() => ({
  '--accent': nightTokens.value['accent-text'], '--accent-text': nightTokens.value['accent-text'],
  '--accent-line': nightTokens.value['accent-line'], '--soft': nightTokens.value.soft,
}));
const colors = computed(() => {
  const palette = REPOSITORY_COLORS.map((color, position) => position < 3 ? nightTokens.value[`repo-${['a', 'b', 'c'][position]}` as 'repo-a' | 'repo-b' | 'repo-c'] : color);
  const ordered = [...props.repositories].sort((left, right) => left.name.localeCompare(right.name));
  return new Map(ordered.map((repo, position) => [repo.id, palette[position % palette.length]]));
});
const repositoryById = computed(() => new Map(props.repositories.map(repo => [repo.id, repo])));
const eligible = computed(() => scopeGraph(index.value, { repositoryId: repositoryId.value, showIsolated: showIsolated.value, range: 0, centerId: null }));
const eligibleIds = computed(() => new Set(eligible.value.nodes.map(node => node.id)));
const effectiveCenter = computed(() => centerId.value && eligibleIds.value.has(centerId.value) ? centerId.value : eligible.value.nodes[0]?.id ?? null);
const scoped = computed(() => scopeGraph(index.value, { repositoryId: repositoryId.value, showIsolated: showIsolated.value, range: range.value, centerId: effectiveCenter.value }));
const visibleIds = computed(() => new Set(scoped.value.nodes.map(node => node.id)));
const positions = computed(() => layoutGraph(scoped.value));
const selectedNode = computed(() => selectedId.value ? index.value.byId.get(selectedId.value) : null);
const selectedNeighbors = computed(() => [...(index.value.adjacency.get(selectedId.value ?? '') ?? [])].map(id => index.value.byId.get(id)!).sort((a, b) => a.title.localeCompare(b.title)));
const repositoryOptions = computed(() => [{ value: '', label: '全部仓库' }, ...props.repositories.map(repo => ({ value: repo.id, label: repo.name }))]);
const labelOptions = ['低', '标准', '高'].map(value => ({ value, label: value }));
const allMatches = computed(() => {
  const term = search.value.trim().toLocaleLowerCase();
  return term ? index.value.nodes.filter(node => `${node.title} ${node.id} ${node.summary} ${node.tags.join(' ')}`.toLocaleLowerCase().includes(term)) : [];
});
const matches = computed(() => allMatches.value.slice(0, 12));
const showMatches = computed(() => searchOpen.value && !!search.value.trim());
const legend = computed(() => {
  const counts = new Map<string, number>();
  for (const node of scoped.value.nodes) counts.set(node.repositoryId, (counts.get(node.repositoryId) ?? 0) + 1);
  return [...counts].map(([id, count]) => ({ id, count, name: repositoryById.value.get(id)?.name ?? id, color: colorByRepository.value ? colors.value.get(id) : 'var(--accent)' }));
});
const selectionStatus = computed(() => selectedNode.value ? `已选中 ${selectedNode.value.title}，高亮 ${selectedNeighbors.value.length} 条直接关联。` : `${range.value ? '关联视图' : '全景'}，${scoped.value.nodes.length} 条记忆，${scoped.value.edges.length} 对关联。`);

function updateScene() {
  renderer?.setScene(scoped.value, positions.value, index.value.degrees, index.value.adjacency);
}

function updateAppearance() {
  renderer?.setAppearance({ selectedId: selectedId.value, hoveredId: hoveredId.value, colors: colors.value, colorByRepository: colorByRepository.value, nodeSize: nodeSize.value, lineWidth: lineWidth.value, labelDensity: labelDensity.value, panelOpen: !!selectedNode.value || settingsOpen.value, showLinks: showLinks.value, searchQuery: search.value.trim() });
}

function refreshTheme() {
  themeVersion.value++;
  void nextTick(() => { renderer?.refreshTheme(); updateAppearance(); });
}

async function loadDetail(id: string) {
  const version = ++detailVersion;
  detailLoading.value = true;
  detailError.value = '';
  detail.value = null;
  try {
    const record = await memoryRead(id);
    if (version === detailVersion) detail.value = record;
  } catch (error) {
    if (version === detailVersion) detailError.value = String(error);
  } finally {
    if (version === detailVersion) detailLoading.value = false;
  }
}

function selectNode(id: string) {
  const node = index.value.byId.get(id);
  if (!node) return;
  if (search.value.trim() && !allMatches.value.some(match => match.id === id)) search.value = '';
  const outsideScope = !visibleIds.value.has(id);
  if (repositoryId.value && node.repositoryId !== repositoryId.value) repositoryId.value = null;
  if (!eligibleIds.value.has(id)) showIsolated.value = true;
  if (outsideScope) { range.value = 0; centerId.value = null; resetCamera(); }
  else if (range.value) { centerId.value = id; resetCamera(); }
  selectedId.value = id;
  hoveredId.value = null;
  settingsOpen.value = false;
  searchOpen.value = false;
  void loadDetail(id);
  void nextTick(() => { if (detailPanel.value) detailPanel.value.scrollTop = 0; });
}

function focusMemory(id: string) {
  if (!index.value.byId.has(id)) return;
  range.value = 0;
  centerId.value = id;
  resetCamera();
  selectNode(id);
}

defineExpose({ focusMemory });

function closeDetail() {
  detailVersion++;
  selectedId.value = null;
  hoveredId.value = null;
  detail.value = null;
  detailLoading.value = false;
  detailError.value = '';
}

function showOverview() {
  range.value = 0;
  centerId.value = null;
  closeDetail();
  resetCamera();
  canvas.value?.focus();
}

function localizeSelected() {
  if (repositoryId.value && selectedNode.value?.repositoryId !== repositoryId.value) repositoryId.value = null;
  centerId.value = selectedId.value;
  range.value = 2;
  resetCamera();
  canvas.value?.focus();
}

function changeRepository(value: string) {
  repositoryId.value = value || null;
  range.value = 0;
  centerId.value = null;
  closeDetail();
  resetCamera();
}

function resetSettings() {
  repositoryId.value = null;
  showIsolated.value = true;
  colorByRepository.value = true;
  showLinks.value = true;
  labelDensity.value = '标准';
  nodeSize.value = 100;
  lineWidth.value = 100;
}

function resetCamera() {
  zoom.value = renderer?.reset() ?? 100;
}

function adjustZoom(factor: number) {
  zoom.value = renderer?.zoomBy(factor) ?? 100;
}

function onWheel(event: WheelEvent) {
  if (!event.deltaY || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
  event.preventDefault();
  adjustZoom(Math.exp(-Math.max(-400, Math.min(400, event.deltaY)) * .002));
}

function pointerDown(event: PointerEvent) {
  if (!event.isPrimary || (event.button !== 0 && event.button !== 1)) return;
  event.preventDefault();
  canvas.value?.focus({ preventScroll: true });
  drag = { pointerId: event.pointerId, button: event.button, x: event.clientX, y: event.clientY, firstX: event.clientX, firstY: event.clientY, moved: false };
  canvas.value?.setPointerCapture(event.pointerId);
}

function pointerMove(event: PointerEvent) {
  if (!drag || drag.pointerId !== event.pointerId) {
    hoveredId.value = renderer?.hit(event.clientX, event.clientY) ?? null;
    return;
  }
  if (!drag.moved && Math.hypot(event.clientX - drag.firstX, event.clientY - drag.firstY) > 3) {
    drag.moved = true;
    dragging.value = true;
  }
  if (drag.moved) {
    hoveredId.value = null;
    if (drag.button === 1) renderer?.pan(event.clientX - drag.x, event.clientY - drag.y);
    else renderer?.rotate(event.clientX - drag.x, event.clientY - drag.y);
  }
  drag.x = event.clientX;
  drag.y = event.clientY;
}

function pointerUp(event: PointerEvent) {
  if (!drag || drag.pointerId !== event.pointerId) return;
  if (!drag.moved && drag.button === 0) {
    const id = renderer?.hit(event.clientX, event.clientY);
    if (id) selectNode(id); else if (selectedId.value || range.value) showOverview();
  }
  drag = null;
  dragging.value = false;
}

function pointerCancel() {
  drag = null;
  dragging.value = false;
}

function canvasKeydown(event: KeyboardEvent) {
  switch (event.key) {
    case 'ArrowLeft': renderer?.rotate(-14, 0); break;
    case 'ArrowRight': renderer?.rotate(14, 0); break;
    case 'ArrowUp': renderer?.rotate(0, -14); break;
    case 'ArrowDown': renderer?.rotate(0, 14); break;
    case '+': case '=': adjustZoom(1.2); break;
    case '-': adjustZoom(1 / 1.2); break;
    case 'Home': resetCamera(); break;
    default: return;
  }
  event.preventDefault();
}

function searchKeydown(event: KeyboardEvent) {
  if (event.isComposing) return;
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    searchOpen.value = true;
    const count = matches.value.length;
    if (count) searchMatchIndex.value = searchMatchIndex.value < 0 ? (event.key === 'ArrowDown' ? 0 : count - 1) : (searchMatchIndex.value + (event.key === 'ArrowDown' ? 1 : count - 1)) % count;
  } else if (event.key === 'Enter' && matches.value.length) {
    event.preventDefault();
    selectSearchMatch(matches.value[Math.max(0, searchMatchIndex.value)].id);
  } else if (event.key === 'Escape') {
    searchOpen.value = false;
    event.stopPropagation();
  }
}

function selectSearchMatch(id: string) {
  resetCamera();
  selectNode(id);
  canvas.value?.focus();
}

function clearSearch() {
  search.value = '';
  searchOpen.value = false;
  searchField.value?.focus();
}

function escape(event: KeyboardEvent) {
  if (event.key !== 'Escape') return;
  searchOpen.value = false;
  if (settingsOpen.value) {
    settingsOpen.value = false;
    settingsButton.value?.focus();
  } else if (selectedId.value) {
    showOverview();
  }
}

watch([scoped, positions, index], updateScene);
watch([selectedId, hoveredId, colors, colorByRepository, showLinks, search, nodeSize, lineWidth, labelDensity, settingsOpen], updateAppearance);
watch(search, () => { searchOpen.value = true; });
watch(matches, () => { searchMatchIndex.value = -1; }, { flush: 'sync' });
watch(eligibleIds, ids => {
  if (selectedId.value && !ids.has(selectedId.value)) closeDetail();
});
watch(() => props.graph, () => {
  if (selectedId.value && index.value.byId.has(selectedId.value)) void loadDetail(selectedId.value);
});
watch(() => props.initialMemoryId, id => { if (id) focusMemory(id); });
watch(() => props.active, active => renderer?.setActive(active));

onMounted(() => {
  renderer = new StarMapRenderer(canvas.value!);
  renderer.setActive(props.active);
  canvas.value!.addEventListener('wheel', onWheel, { passive: false });
  resizeObserver = new ResizeObserver(() => {
    const bounds = canvas.value!.getBoundingClientRect();
    const top = topbar.value!.getBoundingClientRect().bottom - bounds.top + 16;
    panelMaxHeight.value = graphPanelMaxHeight(bounds.height, top);
    renderer?.setTopInset(top);
  });
  resizeObserver.observe(canvas.value!);
  resizeObserver.observe(topbar.value!);
  window.addEventListener('aimemory-theme', refreshTheme);
  updateScene();
  updateAppearance();
  if (props.initialMemoryId) focusMemory(props.initialMemoryId);
});
onBeforeUnmount(() => {
  detailVersion++;
  canvas.value?.removeEventListener('wheel', onWheel);
  resizeObserver?.disconnect();
  window.removeEventListener('aimemory-theme', refreshTheme);
  renderer?.destroy();
});
</script>

<template>
  <section class="star-map" :style="[starTheme, { '--star-panel-max-height': panelMaxHeight + 'px' }]" data-testid="memory-star-map" @keydown="escape">
    <canvas ref="canvas" class="star-canvas" :class="{ dragging, 'node-hovered': hoveredId }" data-testid="star-canvas" :data-range="range" :data-center-id="scoped.centerId" :data-selected-id="selectedId" tabindex="0" aria-label="记忆星图，左键拖动旋转，中键拖动平移，滚轮或加减键缩放，方向键旋转，Home 复位，Escape 关闭详情并返回全景。可通过搜索选择记忆。" aria-describedby="star-map-status" @pointerdown="pointerDown" @pointermove="pointerMove" @pointerup="pointerUp" @pointercancel="pointerCancel" @pointerleave="hoveredId = null" @keydown="canvasKeydown" />
    <div ref="topbar" class="star-topbar">
      <header class="star-heading">
        <div class="star-heading-line"><h1>记忆星图</h1><p class="star-total"><strong>{{ index.nodes.length.toLocaleString() }}</strong> 记忆<span>·</span><strong>{{ index.edges.length.toLocaleString() }}</strong> 关联</p></div>
        <div class="star-legend" aria-label="仓库图例"><span v-for="repo in legend" :key="repo.id" :title="repo.name"><i class="star-dot" :style="{ background: repo.color }" /><span class="star-repository-name">{{ repo.name }}</span><small>{{ repo.count.toLocaleString() }}</small></span></div>
        <div v-if="range > 0" class="star-context"><button class="text-button" @click="showOverview"><Maximize2 :size="14" />返回全景</button><span>当前记忆及直接关联</span></div>
      </header>
      <div class="star-tools">
        <div class="star-search" @focusout="event => { if (!(event.currentTarget as HTMLElement).contains(event.relatedTarget as Node | null)) searchOpen = false; }">
          <Search :size="17" aria-hidden="true" />
          <input ref="searchField" v-model="search" type="search" role="combobox" aria-autocomplete="list" aria-controls="star-search-results" :aria-expanded="showMatches" :aria-activedescendant="searchMatchIndex >= 0 && showMatches ? `star-match-${searchMatchIndex}` : undefined" aria-label="搜索记忆标题、ID、摘要或标签" placeholder="搜索记忆" autocomplete="off" data-testid="star-search" @focus="searchOpen = true" @keydown="searchKeydown" />
          <span v-if="search.trim()" class="star-search-count" data-testid="star-search-count" aria-live="polite">{{ allMatches.length }}</span>
          <button v-if="search" type="button" class="icon-button star-search-clear" aria-label="清空星图搜索" data-testid="star-search-clear" @click="clearSearch"><X :size="14" /></button>
          <div v-if="showMatches" id="star-search-results" class="star-matches" role="listbox" aria-label="匹配的记忆">
            <div class="star-matches-heading">{{ allMatches.length }} 条命中<span v-if="allMatches.length > 12">展示前 12 条</span></div>
            <button v-for="(memory, position) in matches" :id="`star-match-${position}`" :key="memory.id" type="button" role="option" :aria-selected="position === searchMatchIndex" :class="{ active: position === searchMatchIndex }" :data-memory-id="memory.id" @click="selectSearchMatch(memory.id)"><span>{{ memory.title }}</span><small>{{ repositoryById.get(memory.repositoryId)?.name ?? memory.repositoryId }} · {{ memory.id }}<em v-if="!visibleIds.has(memory.id)">当前范围外</em></small></button>
            <p v-if="!matches.length">没有匹配的记忆</p>
          </div>
        </div>
        <button ref="settingsButton" class="button star-settings-button" :class="{ active: settingsOpen }" :aria-expanded="settingsOpen" aria-controls="star-settings" data-testid="star-settings-toggle" @click="settingsOpen = !settingsOpen; searchOpen = false"><Settings2 :size="17" aria-hidden="true" />图谱设置<ChevronDown :size="14" aria-hidden="true" /></button>
      </div>
    </div>

    <aside v-if="settingsOpen" id="star-settings" class="star-panel star-settings" aria-label="图谱设置" data-testid="star-settings" @wheel.stop>
      <div class="star-panel-heading"><h2>图谱设置</h2><button class="icon-button" aria-label="关闭图谱设置" @click="settingsOpen = false; settingsButton?.focus()"><X :size="17" /></button></div>
      <section class="star-setting-section"><h3>筛选</h3><div class="star-option"><span>仓库范围</span><AppSelect :model-value="repositoryId ?? ''" :options="repositoryOptions" aria-label="筛选仓库" data-testid="star-repository-filter" @update:model-value="changeRepository" /></div><label class="star-option"><span>显示孤立记忆</span><input v-model="showIsolated" type="checkbox" role="switch" data-testid="star-isolated" /></label></section>
      <section class="star-setting-section"><h3>分组</h3><label class="star-option"><span>按仓库着色</span><span class="star-color-toggle"><i v-for="color in [...colors.values()].slice(0, 4)" :key="color" :style="{ background: color }" /><input v-model="colorByRepository" type="checkbox" role="switch" /></span></label></section>
      <section class="star-setting-section"><h3>显示</h3><label class="star-option"><span>关系线</span><input v-model="showLinks" type="checkbox" role="switch" data-testid="star-show-links" /></label><div class="star-option"><span>标签密度</span><AppSelect :model-value="labelDensity" :options="labelOptions" aria-label="标签密度" @update:model-value="value => { labelDensity = value as GraphLabelDensity; }" /></div><label class="star-slider"><span>节点大小</span><output>{{ nodeSize }}%</output><input v-model.number="nodeSize" type="range" min="50" max="180" step="10" aria-label="节点大小" /></label><label class="star-slider" :class="{ disabled: !showLinks }"><span>连线粗细</span><output>{{ lineWidth }}%</output><input v-model.number="lineWidth" type="range" min="50" max="180" step="10" :disabled="!showLinks" aria-label="连线粗细" /></label></section>
      <footer><button class="text-button" @click="resetSettings"><RefreshCw :size="15" />恢复默认</button></footer>
    </aside>

    <aside v-else-if="selectedNode" ref="detailPanel" class="star-panel star-detail" aria-label="所选记忆" data-testid="star-detail" @wheel.stop>
      <div class="star-panel-heading"><span class="star-selected-chip">记忆原文</span><button class="icon-button" aria-label="关闭节点详情" title="关闭详情，返回全景" @click="showOverview"><X :size="17" /></button></div>
      <h2>{{ selectedNode.title }}</h2>
      <div class="star-detail-meta"><span class="star-dot" :style="{ background: colorByRepository ? colors.get(selectedNode.repositoryId) : 'var(--accent)' }" /><span>{{ selectedNode.id }}</span><span>·</span><span>{{ repositoryById.get(selectedNode.repositoryId)?.name ?? selectedNode.repositoryId }}</span></div>
      <p v-if="!detail" class="star-summary">{{ selectedNode.summary }}</p>
      <p v-if="detailLoading" class="star-read-status" role="status">正在读取本地 Markdown…</p>
      <p v-else-if="detailError" class="star-read-error" role="alert">{{ detailError }}</p>
      <div v-else-if="detail" class="star-body"><MarkdownView :markdown="detail.markdown" :title="selectedNode.title" /><p v-if="detail.source" class="star-source">来源 · {{ detail.source }}</p></div>
      <div class="star-detail-actions"><button class="button" @click="emit('open-memory', selectedNode.id)"><FileText :size="15" />查看完整记忆</button><button v-if="!range" class="button" @click="localizeSelected"><Focus :size="15" />只看关联</button><button v-else class="button" @click="showOverview"><Maximize2 :size="15" />返回全景</button></div>
      <div class="star-related-heading"><Link2 :size="17" /><h3>直接关联</h3><span>{{ selectedNeighbors.length }} 条</span></div>
      <div class="star-neighbors"><button v-for="neighbor in selectedNeighbors" :key="neighbor.id" class="star-neighbor" :data-memory-id="neighbor.id" @click="selectNode(neighbor.id)"><span class="star-dot" :style="{ background: colorByRepository ? colors.get(neighbor.repositoryId) : 'var(--accent)' }" /><span><strong>{{ neighbor.title }}</strong><small>{{ neighbor.id }} · {{ repositoryById.get(neighbor.repositoryId)?.name ?? neighbor.repositoryId }}</small></span><ChevronRight :size="15" /></button><p v-if="!selectedNeighbors.length">这条记忆尚未建立关联。</p></div>
    </aside>

    <div v-if="!graph.nodes.length" class="star-empty"><GitBranch :size="32" /><h2>还没有可显示的记忆</h2></div>
    <div v-else-if="!scoped.nodes.length" class="star-empty"><Focus :size="30" /><h2>当前范围没有记忆</h2></div>
    <div class="star-camera"><button class="icon-button" aria-label="缩小星图" :disabled="zoom <= STAR_MAP_MIN_ZOOM * 100" @click="adjustZoom(1 / 1.2)"><Minus :size="17" /></button><output aria-label="缩放比例">{{ zoom }}%</output><button class="icon-button" aria-label="放大星图" :disabled="zoom >= STAR_MAP_MAX_ZOOM * 100" @click="adjustZoom(1.2)"><Plus :size="17" /></button><span /><button class="icon-button" aria-label="重置星图视角" title="重置视角" @click="resetCamera"><Maximize2 :size="17" /></button></div>
    <footer class="star-footer"><div id="star-map-status" class="star-count" role="status" aria-live="polite" data-testid="star-range-status"><strong>{{ range ? '关联视图' : '全景' }}</strong><span>{{ scoped.nodes.length.toLocaleString() }} 记忆 · {{ scoped.edges.length.toLocaleString() }} 关联</span></div><span class="star-gesture">左键旋转 · 中键平移 · 滚轮缩放 · 点击阅读</span><span class="star-token">本地浏览</span></footer>
    <span class="star-sr" aria-live="polite">{{ selectionStatus }}</span>
  </section>
</template>

<style scoped>
.star-map { --bg: #0e1a2d; --fg: #e4edf9; --background: var(--bg); --foreground: var(--fg); --surface: #13213a; --side: #0d1a2d; --muted: #a2b3cb; --faint: #7e91ad; --line: #263952; --hover: #172a45; --graph-bg: #080f1d; --graph-line: #2b4264; --graph-muted: #445672; --danger: #f3a5a1; position: relative; flex: 1; min-height: 0; overflow: hidden; background: var(--graph-bg); color: var(--fg); color-scheme: dark; container-type: inline-size; container-name: star-map; }
.star-canvas { position: absolute; inset: 0; display: block; width: 100%; height: 100%; touch-action: none; cursor: grab; outline-offset: -4px; }
.star-canvas.dragging { cursor: grabbing; }
.star-canvas.node-hovered { cursor: pointer; }
.star-map .button { color: var(--fg); background: #13233a; border-color: var(--line); }
.star-map .button:hover { background: #1a304e; border-color: var(--accent-line); }
.star-map .icon-button:hover, .star-map .text-button:hover { background: var(--hover); color: var(--fg); }
.star-topbar { position: absolute; z-index: 4; left: 22px; right: 22px; top: 20px; display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: start; gap: 16px; pointer-events: none; }
.star-heading { min-width: 0; max-width: 640px; }
.star-heading-line { display: flex; flex-wrap: wrap; align-items: baseline; gap: 6px 14px; }
.star-heading h1 { font-size: 19px; line-height: 1.4; font-weight: 600; letter-spacing: -.35px; white-space: nowrap; }
.star-total { display: flex; align-items: baseline; flex-wrap: wrap; gap: 5px; color: var(--faint); font-size: 11px; line-height: 1.6; font-variant-numeric: tabular-nums; }
.star-total strong { color: #becde2; font-weight: 500; }
.star-total > span { margin: 0 3px; color: #536780; }
.star-legend { display: flex; flex-wrap: wrap; gap: 5px 13px; margin-top: 12px; padding: 7px 10px; width: fit-content; max-width: 100%; background: #0e1b2b9c; border: 1px solid #2539518a; border-radius: 7px; font-size: 10px; line-height: 1.5; color: var(--muted); pointer-events: auto; backdrop-filter: blur(12px); }
.star-legend:empty { display: none; }
.star-legend > span { display: inline-flex; flex: 0 1 auto; gap: 6px; align-items: center; min-width: 0; max-width: 100%; }
.star-repository-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.star-legend small { flex-shrink: 0; font-size: 10px; color: var(--faint); font-variant-numeric: tabular-nums; }
.star-tools { display: flex; gap: 8px; min-width: 0; pointer-events: auto; }
.star-search { position: relative; display: flex; align-items: center; gap: 8px; width: 236px; height: 36px; border: 1px solid #2a3e59; border-radius: 8px; background: #0e1a2de8; padding: 0 10px; color: var(--muted); backdrop-filter: blur(14px); }
.star-search:focus-within { border-color: var(--accent-line); box-shadow: 0 0 0 2px var(--soft); }
.star-search input { min-width: 0; flex: 1; width: 100%; height: 100%; padding: 0; background: transparent; color: var(--fg); border: 0; outline: 0; font-size: 12px; }
.star-search input::placeholder { color: var(--faint); }
.star-search input::-webkit-search-cancel-button { appearance: none; }
.star-search-count { font: 10px Menlo, Consolas, monospace; color: var(--accent-text); }
.star-search-clear { width: 21px; height: 23px; flex-shrink: 0; }
.star-map .star-settings-button { flex-shrink: 0; min-height: 36px; padding: 7px 10px; gap: 7px; font-size: 11px; background: #0e1a2de8; backdrop-filter: blur(14px); }
.star-settings-button > svg:last-child { margin-left: 2px; }
.star-map .star-settings-button.active { background: var(--soft); border-color: var(--accent-line); color: var(--accent-text); }
.star-matches { position: absolute; z-index: 8; left: -1px; right: -1px; top: 42px; max-height: min(340px, calc(100vh - 250px)); overflow: auto; padding: 5px; background: #101d30f5; border: 1px solid #2c4260; border-radius: 8px; box-shadow: 0 12px 32px #0005; backdrop-filter: blur(16px); }
.star-matches-heading { display: flex; justify-content: space-between; gap: 8px; padding: 7px 8px 9px; color: var(--muted); font-size: 10px; }
.star-matches-heading > span { color: var(--faint); }
.star-matches button { display: flex; width: 100%; gap: 5px; flex-direction: column; align-items: flex-start; padding: 9px 8px; text-align: left; color: var(--fg); border: 0; border-radius: 5px; background: transparent; }
.star-matches button:hover, .star-matches button.active { background: var(--soft); color: var(--accent-text); }
.star-matches button > span { font-size: 12px; line-height: 1.5; overflow-wrap: anywhere; }
.star-matches small { color: var(--faint); font: 9px/1.7 Menlo, Consolas, monospace; overflow-wrap: anywhere; }
.star-matches em { font-size: 9px; font-style: normal; line-height: 1.7; color: var(--muted); margin-left: 8px; }
.star-matches p { color: var(--muted); font-size: 12px; padding: 14px 8px; line-height: 1.7; }
.star-context { display: flex; align-items: center; flex-wrap: wrap; gap: 6px 10px; width: fit-content; max-width: 100%; margin-top: 12px; pointer-events: auto; padding: 4px 9px 4px 4px; border: 1px solid #2a3e59; border-radius: 8px; background: #0e1a2de8; color: var(--faint); font-size: 10px; backdrop-filter: blur(14px); }
.star-context .text-button { color: var(--accent-text); font-size: 11px; }
.star-panel { position: absolute; z-index: 3; right: 22px; top: 72px; width: 324px; max-height: calc(100% - 138px); overflow-y: auto; overscroll-behavior: contain; background: #0e1a2df0; color: var(--fg); border: 1px solid #2c405b; border-radius: 10px; box-shadow: 0 12px 40px #0004; backdrop-filter: blur(18px); scrollbar-color: #344a68 transparent; }
.star-panel-heading { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.star-panel-heading .icon-button { width: 27px; height: 27px; background: #15243a; }
.star-panel-heading h2 { font-size: 15px; font-weight: 550; }
.star-selected-chip { color: var(--accent-text); background: var(--soft); border: 1px solid var(--accent-line); border-radius: 5px; padding: 4px 7px; font-size: 10px; font-weight: 500; }
.star-detail { padding: 14px 16px; }
.star-detail > h2 { font-size: 18px; font-weight: 600; line-height: 1.5; margin: 17px 0 11px; letter-spacing: -.2px; overflow-wrap: anywhere; }
.star-detail-meta { display: flex; align-items: center; flex-wrap: wrap; gap: 7px; color: var(--faint); font: 9px/1.8 Menlo, Consolas, monospace; }
.star-detail-meta > span:last-child { overflow-wrap: anywhere; }
.star-summary { margin-top: 18px; font-size: 13px; line-height: 1.85; color: var(--muted); }
.star-read-status, .star-read-error { margin-top: 14px; font-size: 12px; line-height: 1.7; }
.star-read-status { color: var(--muted); }
.star-read-error { color: var(--danger); overflow-wrap: anywhere; }
.star-body { margin-top: 18px; }
.star-body :deep(.markdown) { color: #c4d1e4; font-size: 12px; line-height: 1.85; }
.star-body :deep(.markdown h1), .star-body :deep(.markdown h2), .star-body :deep(.markdown h3) { color: var(--fg); font-size: 13px; margin: 20px 0 9px; line-height: 1.6; }
.star-body :deep(.markdown .memory-conclusion) { padding: 11px 12px; margin: 0 0 17px; border-radius: 0 6px 6px 0; border-left-width: 2px; }
.star-body :deep(.markdown .memory-conclusion h2) { margin-top: 0; font-size: 11px; }
.star-body :deep(.markdown .memory-conclusion p) { font-size: 12px; font-weight: 400; line-height: 1.85; }
.star-body :deep(.markdown blockquote) { padding: 10px 12px; margin: 12px 0; border-left-width: 2px; }
.star-body :deep(.markdown p) { margin: 9px 0; }
.star-body :deep(.markdown pre) { padding: 10px; }
.star-source { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--line); color: var(--faint); font-size: 10px; line-height: 1.8; overflow-wrap: anywhere; }
.star-detail-actions { display: flex; gap: 7px; margin: 17px 0 16px; }
.star-detail-actions .button { flex: 1; min-width: 0; padding: 7px 8px; font-size: 10px; gap: 5px; }
.star-related-heading { display: flex; align-items: center; gap: 7px; padding: 15px 0 7px; border-top: 1px solid var(--line); }
.star-related-heading > svg { color: var(--accent); }
.star-related-heading h3 { font-size: 13px; font-weight: 550; }
.star-related-heading > span { margin-left: auto; font-size: 10px; color: var(--faint); }
.star-neighbor { display: flex; align-items: flex-start; gap: 9px; width: 100%; border: 0; color: var(--fg); background: transparent; padding: 9px 2px; text-align: left; border-radius: 5px; }
.star-neighbor:hover { background: var(--soft); }
.star-neighbor > .star-dot { margin-top: 6px; }
.star-neighbor > span:nth-child(2) { flex: 1; min-width: 0; }
.star-neighbor strong { display: block; font-size: 12px; font-weight: 500; line-height: 1.6; overflow-wrap: anywhere; }
.star-neighbor small { display: block; font: 9px/1.7 Menlo, Consolas, monospace; color: var(--faint); margin-top: 4px; overflow-wrap: anywhere; }
.star-neighbor > svg { margin-top: 5px; color: var(--faint); flex-shrink: 0; }
.star-neighbors > p { color: var(--faint); font-size: 12px; line-height: 1.8; padding: 8px 0; }
.star-settings .star-panel-heading { padding: 13px 16px; border-bottom: 1px solid var(--line); }
.star-setting-section { padding: 14px 17px 1px; }
.star-setting-section + .star-setting-section { border-top: 1px solid var(--line); margin-top: 12px; }
.star-setting-section h3 { color: var(--muted); font-size: 11px; font-weight: 500; margin-bottom: 13px; }
.star-option { display: flex; align-items: center; justify-content: space-between; gap: 12px; font-size: 12px; margin: 14px 0; }
.star-option > span:first-child { flex-shrink: 0; }
.star-option :deep(.app-select) { min-width: 90px; max-width: 175px; min-height: 32px; padding: 0 8px; border-radius: 6px; font-size: 11px; background: #13233a; }
.star-option input[type=checkbox] { appearance: none; width: 32px; height: 19px; border-radius: 10px; background: #34445d; position: relative; margin: 0; flex-shrink: 0; cursor: pointer; }
.star-option input[type=checkbox]::after { content: ''; position: absolute; left: 3px; top: 3px; width: 13px; height: 13px; background: #dce8f7; border-radius: 50%; transition: transform .13s; }
.star-option input[type=checkbox]:checked { background: var(--accent); }
.star-option input[type=checkbox]:checked::after { background: #102039; transform: translateX(13px); }
.star-color-toggle { display: flex; gap: 5px; align-items: center; }
.star-color-toggle > i { width: 6px; height: 6px; border-radius: 50%; }
.star-color-toggle input { margin-left: 8px; }
.star-slider { display: flex; flex-wrap: wrap; justify-content: space-between; row-gap: 10px; font-size: 12px; margin: 19px 0; }
.star-slider.disabled { opacity: .5; }
.star-slider output { color: var(--faint); font: 10px Menlo, Consolas, monospace; }
.star-slider input { width: 100%; height: 15px; accent-color: var(--accent); cursor: pointer; margin: 0; }
.star-slider input:disabled { cursor: default; }
.star-settings footer { border-top: 1px solid var(--line); margin-top: 8px; padding: 6px 11px; }
.star-settings footer .text-button { font-size: 11px; }
.star-dot { display: inline-block; width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }
.star-camera { display: flex; align-items: center; gap: 3px; position: absolute; right: 22px; bottom: 16px; height: 35px; background: #0e1a2de8; border: 1px solid #2a3e59; border-radius: 8px; padding: 3px 6px; backdrop-filter: blur(14px); }
.star-camera output { min-width: 39px; text-align: center; font: 10px Menlo, Consolas, monospace; color: var(--muted); }
.star-camera > span { height: 17px; border-left: 1px solid var(--line); margin: 0 4px; }
.star-footer { display: flex; align-items: center; flex-wrap: wrap; gap: 5px 18px; position: absolute; bottom: 16px; left: 22px; right: 212px; min-height: 35px; padding: 7px 10px; background: #0e1a2dba; border: 1px solid #2539518a; border-radius: 8px; font-size: 10px; color: var(--muted); line-height: 1.7; backdrop-filter: blur(12px); }
.star-count { display: flex; gap: 10px; flex-wrap: wrap; font-variant-numeric: tabular-nums; }
.star-count strong { font-weight: 500; color: #c3d3e8; }
.star-gesture { margin-left: auto; color: var(--faint); }
.star-token { color: var(--faint); font-size: 9px; }
.star-empty { position: absolute; top: 46%; left: 50%; transform: translate(-50%, -50%); width: min(360px, calc(100% - 48px)); text-align: center; color: var(--faint); pointer-events: none; }
.star-empty h2 { font-size: 17px; font-weight: 500; color: #becde2; margin-top: 18px; }
.star-empty p { font-size: 12px; line-height: 1.9; margin-top: 12px; }
.star-sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
@container star-map (max-width: 1040px) {
  .star-gesture, .star-token { display: none; }
}
@container star-map (max-width: 760px) {
  .star-topbar { left: 18px; right: 18px; top: 18px; grid-template-columns: minmax(0, 1fr); gap: 14px; }
  .star-heading { max-width: none; }
  .star-search { flex: 1; width: auto; min-width: 0; }
  .star-settings-button > svg:last-child { display: none; }
  .star-panel { left: 18px; right: 18px; top: auto; bottom: 63px; width: auto; max-height: var(--star-panel-max-height); }
  .star-context > span { display: none; }
  .star-camera { bottom: 14px; right: 18px; }
  .star-footer { left: 18px; right: 205px; bottom: 14px; }
  .star-count { gap: 2px 8px; }
}
@container star-map (max-width: 560px) {
  .star-total { font-size: 10px; }
}
@media (prefers-reduced-motion: reduce) {
  .star-option input[type=checkbox]::after { transition: none; }
}
</style>
