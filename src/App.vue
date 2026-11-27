<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { Activity, ArrowLeft, ArrowRight, BookOpen, Check, ChevronRight, Clock3, Database, FileText, FolderOpen, GitBranch, History, Link2, Orbit, PanelLeft, Plus, RefreshCw, Search, Settings2, SquarePen, Trash2, X } from 'lucide-vue-next';
import { archiveRetry, archiveStatus, chooseWorkspace, memoryGraph, memoryRead, memorySave, memoryAdoptSuggestion, memorySearch, openMemoryFile, repositoryAdd, repositoryDelete, repositoryRename, semanticSearch, settingsGet, settingsSave, workspaceMigrate, workspaceOpen, workspaceRefresh } from './api';
import type { AppSettings, ArchiveStatus, MemoryGraph, MemoryInput, MemoryRecord, MemorySummary, Repository, SearchHit, SearchResponse, SettingsInput, Workspace } from './types';
import MarkdownView from './components/MarkdownView.vue';
import MemoryEditor from './components/MemoryEditor.vue';
import MemoryProvenance from './components/MemoryProvenance.vue';
import MemoryUpdateLog from './components/MemoryUpdateLog.vue';
import SettingsPanel from './components/SettingsPanel.vue';
import ArchiveUsagePage from './components/ArchiveUsagePage.vue';
import ArchiveSidebarStatus from './components/ArchiveSidebarStatus.vue';
import { getArchiveSidebarState } from './archive-sidebar';
import { installSmoothWheelScroll } from './smooth-scroll';
import MemoryStarMap from './components/MemoryStarMap.vue';
import { REPOSITORY_COLORS } from './graph';
import ThemePanel from './components/ThemePanel.vue';
import TransferPanel from './components/TransferPanel.vue';
import { applyTheme, DEFAULT_THEME, themeGet, themeSave } from './theme';
import type { ThemeConfig } from './theme';
import type { TransferApplyResult } from './transfer';
import brandMark from '../src-tauri/icons/128x128.png';

const workspace = ref<Workspace | null>(null);
const view = ref<'workbench' | 'graph' | 'editor' | 'settings' | 'archive'>('workbench');
const theme = shallowRef<ThemeConfig>({ ...DEFAULT_THEME });
const savingTheme = ref(false);
const transferPanel = ref<InstanceType<typeof TransferPanel>>();
let themeVersion = 0;
let themeWrites: Promise<void> = Promise.resolve();
const graph = shallowRef<MemoryGraph | null>(null);
const starMap = ref<InstanceType<typeof MemoryStarMap>>();
const graphFocusId = ref<string | null>(null);
const graphLoading = ref(false);
const graphReaderActive = ref(false);
let graphStale = true;
let graphVersion = 0;
let workbenchNeedsRefresh = false;
const appSettings = ref<AppSettings | null>(null);
const archive = ref<ArchiveStatus | null>(null);
const archiveTab = ref<'usage' | 'archive'>('usage');
const archiveReadError = ref('');
const archiveSidebarState = computed(() => getArchiveSidebarState(archive.value, !!archiveReadError.value));
const settingsPanel = ref<InstanceType<typeof SettingsPanel>>();
const savingSettings = ref(false);
const retryingArchive = ref(false);
const activeRepositoryId = ref<string | null>(null);
const query = ref('');
const searchMode = ref<'semantic' | 'keyword'>('semantic');
const semanticResult = ref<SearchResponse | null>(null);
const searchError = ref('');
const results = ref<(MemorySummary & Partial<SearchHit>)[]>([]);
const expandedGroup = ref<string | null>(null);
const groupResult = ref<SearchResponse | null>(null);
const groupLoading = ref(false);
const groupError = ref('');
const adopting = ref(false);
const allMemories = ref<MemorySummary[]>([]);
const selectedMemory = ref<MemoryRecord | null>(null);
const updateLogOpen = ref(false);
const editingMemory = ref<MemoryRecord | null>(null);
const editorKey = ref(0);
const editor = ref<InstanceType<typeof MemoryEditor>>();
const searchInput = ref<HTMLInputElement>();
const resultScroll = ref<HTMLElement>();
const readerTitle = ref<HTMLElement>();
const loading = ref(true);
const changingWorkspace = ref(false);
const searching = ref(false);
const reading = ref(false);
const saving = ref(false);
const error = ref('');
const status = ref('');
const narrowReader = ref(false);
const sidebarOpen = ref(false);
const sidebarCollapsed = ref(false);
const sidebarToggle = ref<HTMLButtonElement>();
const sidebarMedia = window.matchMedia('(max-width: 700px)');
const compactSidebar = ref(sidebarMedia.matches);
const sidebarExpanded = computed(() => compactSidebar.value ? sidebarOpen.value : !sidebarCollapsed.value);
const relatedHistory = ref<string[]>([]);
const pendingNavigation = ref<(() => void) | null>(null);
const repositoryDialog = ref(false);
const repositoryName = ref('');
const repositoryRemote = ref('');
const repositoryError = ref('');
const addingRepository = ref(false);
const repositoryMenu = ref<{ repository: Repository; x: number; y: number } | null>(null);
const repositoryAction = ref<{ kind: 'rename' | 'delete'; repository: Repository } | null>(null);
const actionName = ref('');
const actionError = ref('');
const savingRepository = ref(false);
const repositoryMenuElement = ref<HTMLElement>();
const repositoryActionInput = ref<HTMLInputElement>();
const modalOpen = computed(() => !!(repositoryAction.value || repositoryDialog.value || pendingNavigation.value));
let repositoryMenuTrigger: HTMLElement | undefined;
let modalTrigger: HTMLElement | undefined;
let searchVersion = 0;
let readVersion = 0;
let searchTimer: ReturnType<typeof setTimeout>;
let statusTimer: ReturnType<typeof setTimeout>;
let archiveTimer: ReturnType<typeof setInterval>;
let pollingArchive = false;
let archiveRevision = '';
let lastMemoryWrites: number | null = null;
let unlistenClose: UnlistenFn | undefined;
let stopSmoothWheelScroll: (() => void) | undefined;
let discardOnClose = false;

const repositories = computed(() => workspace.value?.repositories ?? []);
const activeRepository = computed(() => repositories.value.find(repo => repo.id === activeRepositoryId.value));
const currentRepository = computed(() => repositories.value.find(repo => repo.id === selectedMemory.value?.repositoryId));
const totalMemories = computed(() => repositories.value.reduce((sum, repo) => sum + repo.memoryCount, 0));
const relatedMemories = computed(() => selectedMemory.value?.relatedIds.map(id => ({ id, memory: allMemories.value.find(memory => memory.id === id) })) ?? []);
const pageTitle = computed(() => {
  const page = view.value;
  if (page === 'editor') return editingMemory.value ? '编辑记忆' : '新增记忆';
  return { workbench: '记忆工作台', graph: '记忆星图', settings: '设置与接入', archive: '归档与用量', theme: '主题与外观', transfer: '导入与导出' }[page];
});
const pendingVectors = computed(() => allMemories.value.filter(memory =>
  (!activeRepositoryId.value || memory.repositoryId === activeRepositoryId.value) &&
  ['unconfigured', 'pending', 'processing'].includes(memory.embeddingStatus)).length);
const semanticActive = computed(() => query.value.trim().length > 0 && searchMode.value === 'semantic');
const searchLabel = computed(() => semanticResult.value ? ({ keyword: '关键词匹配', semantic: '语义相关度', hybrid: '综合排序', id: '精确 ID', group: '同组记忆' }[semanticResult.value.searchMode]) : '');
const canUseNative = isTauri();

function notify(message: string) {
  status.value = message;
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => { status.value = ''; }, 4500);
}

function navigate(action: () => void) {
  if (transferPanel.value?.busy || changingWorkspace.value || savingRepository.value) return;
  if (view.value === 'editor' && editor.value?.dirty || view.value === 'settings' && settingsPanel.value?.dirty) {
    pendingNavigation.value = action;
    return;
  }
  action();
  if (sidebarOpen.value) dismissSidebar();
}

function changeTheme(next: ThemeConfig) {
  const version = ++themeVersion;
  theme.value = next;
  applyTheme(next);
  savingTheme.value = true;
  themeWrites = themeWrites.then(async () => {
    try {
      const saved = await themeSave(next);
      if (version === themeVersion) theme.value = saved;
    } catch (cause) {
      error.value = String(cause);
    } finally {
      if (version === themeVersion) savingTheme.value = false;
    }
  });
}

function showSettingsSection(section: 'settings') {
  navigate(() => { view.value = section; });
}

function openArchiveDestination(target: 'archive' | 'settings') {
  if (target === 'archive') archiveTab.value = 'archive';
  if (view.value === target) {
    sidebarOpen.value = false;
    return;
  }
  navigate(() => { view.value = target; });
}

async function importedTransfer(result: TransferApplyResult) {
  if (result.theme) {
    theme.value = result.theme;
    applyTheme(result.theme);
  }
  if (result.settings) appSettings.value = result.settings;
  workbenchNeedsRefresh = true;
  if (result.kind === 'memories') {
    loading.value = true;
    try {
      workspace.value = result.workspace;
      allMemories.value = await memorySearch(null, '');
      await invalidateGraph();
      if (selectedMemory.value) await readMemory(selectedMemory.value.id, false);
    } catch (cause) {
      error.value = String(cause);
    } finally {
      loading.value = false;
    }
  }
  await updateArchive();
}

function discardAndNavigate() {
  const action = pendingNavigation.value;
  pendingNavigation.value = null;
  if (view.value === 'settings') settingsPanel.value?.reset();
  action?.();
  sidebarOpen.value = false;
}

function toggleSidebar() {
  if (compactSidebar.value) sidebarOpen.value = !sidebarOpen.value;
  else sidebarCollapsed.value = !sidebarCollapsed.value;
}

function dismissSidebar() {
  sidebarOpen.value = false;
  sidebarToggle.value?.focus();
}

function updateSidebarLayout(event: MediaQueryListEvent) {
  compactSidebar.value = event.matches;
  sidebarOpen.value = false;
}

function showWorkbench(repositoryId = activeRepositoryId.value) {
  navigate(() => {
    view.value = 'workbench';
    graphReaderActive.value = false;
    narrowReader.value = false;
    if (activeRepositoryId.value !== repositoryId) {
      activeRepositoryId.value = repositoryId;
      query.value = '';
      relatedHistory.value = [];
    }
    if (workbenchNeedsRefresh) {
      workbenchNeedsRefresh = false;
      void searchMemories();
    }
  });
}

async function loadGraph() {
  const version = ++graphVersion;
  graphLoading.value = true;
  try {
    const result = await memoryGraph();
    if (version !== graphVersion) return;
    graph.value = result;
    graphStale = false;
  } catch (cause) {
    if (version === graphVersion) error.value = String(cause);
  } finally {
    if (version === graphVersion) graphLoading.value = false;
  }
}

function showStarMap(focusId: string | null = view.value === 'workbench' ? selectedMemory.value?.id ?? null : null) {
  navigate(() => {
    clearTimeout(searchTimer);
    if (searching.value) workbenchNeedsRefresh = true;
    searchVersion++;
    searching.value = false;
    view.value = 'graph';
    graphReaderActive.value = false;
    if (focusId) graphFocusId.value = focusId;
    if (graphStale) void loadGraph();
    if (focusId) void nextTick(() => starMap.value?.focusMemory(focusId));
  });
}

async function invalidateGraph() {
  graphStale = true;
  if (view.value === 'graph') await loadGraph();
}

function openGraphMemory(id: string) {
  view.value = 'workbench';
  graphReaderActive.value = true;
  relatedHistory.value = [];
  void readMemory(id);
}

async function readMemory(id: string, reveal = true) {
  const version = ++readVersion;
  reading.value = true;
  try {
    const memory = await memoryRead(id);
    if (version !== readVersion) return;
    selectedMemory.value = memory;
    if (reveal) {
      narrowReader.value = true;
      if (window.innerWidth <= 1050) {
        await nextTick();
        readerTitle.value?.focus({ preventScroll: true });
      }
    }
  } catch (cause) {
    if (version === readVersion) error.value = String(cause);
  } finally {
    if (version === readVersion) reading.value = false;
  }
}

async function searchMemories() {
  clearTimeout(searchTimer);
  const version = ++searchVersion;
  searching.value = true;
  searchError.value = '';
  try {
    let memories: (MemorySummary & Partial<SearchHit>)[];
    expandedGroup.value = null;
    groupResult.value = null;
    if (query.value.trim()) {
      const response = await semanticSearch({ query: query.value.trim(), repositoryId: activeRepositoryId.value, limit: 5, mode: searchMode.value === 'keyword' ? 'keyword' : 'auto' });
      if (version !== searchVersion) return;
      semanticResult.value = response;
      memories = response.results;
    } else {
      memories = await memorySearch(activeRepositoryId.value, query.value);
      if (version !== searchVersion) return;
      semanticResult.value = null;
    }
    if (version !== searchVersion) return;
    results.value = memories;
    workbenchNeedsRefresh = false;
    if (!graphReaderActive.value && !relatedHistory.value.length && !memories.some(memory => memory.id === selectedMemory.value?.id)) {
      selectedMemory.value = null;
      readVersion++;
      if (memories[0]) await readMemory(memories[0].id, false);
    }
  } catch (cause) {
    if (version === searchVersion) {
      searchError.value = String(cause);
      results.value = [];
      semanticResult.value = null;
    }
  } finally {
    if (version === searchVersion) searching.value = false;
  }
}

function submitSearch(event?: KeyboardEvent) {
  if (event?.isComposing) return;
  event?.preventDefault();
  if (workspace.value && !loading.value) void searchMemories();
}

function clearSearch() {
  query.value = '';
  searchInput.value?.focus();
}

function moveSearchFocus(event: KeyboardEvent) {
  if (event.isComposing || searching.value) return;
  const buttons = Array.from(resultScroll.value?.querySelectorAll<HTMLButtonElement>('.memory-result') ?? []);
  if (!buttons.length) return;
  const current = buttons.indexOf(event.currentTarget as HTMLButtonElement);
  const down = event.key === 'ArrowDown';
  const next = current < 0 ? (down ? 0 : buttons.length - 1) : current + (down ? 1 : -1);
  const target = next < 0 ? searchInput.value : buttons[next];
  if (!target) return;
  event.preventDefault();
  target.focus();
}

function useKeywordSearch() {
  searchMode.value = 'keyword';
}

async function expandGroup(memory: MemorySummary, offset = 0) {
  expandedGroup.value = memory.id;
  groupLoading.value = true;
  groupError.value = '';
  try {
    const result = await semanticSearch({ repositoryId: memory.repositoryId, groupOf: memory.id, offset, limit: 5 });
    if (expandedGroup.value === memory.id) groupResult.value = result;
  } catch (cause) {
    if (expandedGroup.value === memory.id) groupError.value = String(cause);
  } finally {
    if (expandedGroup.value === memory.id) groupLoading.value = false;
  }
}

async function adoptSuggestion(sourceId: string) {
  const current = selectedMemory.value;
  if (!current || adopting.value) return;
  adopting.value = true;
  error.value = '';
  try {
    const saved = await memoryAdoptSuggestion(current.id, sourceId, current.revision);
    allMemories.value = await memorySearch(null, '');
    await invalidateGraph();
    await searchMemories();
    selectedMemory.value = saved;
    notify('已采用建议，当前结论记为人工确认');
  } catch (cause) {
    error.value = String(cause);
    if (selectedMemory.value?.id === current.id) await readMemory(current.id, false);
  } finally {
    adopting.value = false;
  }
}

async function loadWorkspace(path?: string, migrate = false) {
  loading.value = true;
  error.value = '';
  try {
    const next = path && migrate ? await workspaceMigrate(path) : await workspaceOpen(path);
    clearTimeout(searchTimer);
    searchVersion++;
    readVersion++;
    searchError.value = '';
    semanticResult.value = null;
    searching.value = false;
    graphVersion++;
    graph.value = null;
    graphLoading.value = false;
    graphStale = true;
    if (!migrate) {
      graphFocusId.value = null;
      graphReaderActive.value = false;
    }
    workspace.value = next;
    archive.value = null;
    archiveReadError.value = '';
    archiveRevision = '';
    lastMemoryWrites = null;
    allMemories.value = await memorySearch(null, '');
    await invalidateGraph();
    if (!migrate) {
      activeRepositoryId.value = repositories.value[0]?.id ?? null;
      selectedMemory.value = null;
      relatedHistory.value = [];
      query.value = '';
    } else if (selectedMemory.value) {
      await readMemory(selectedMemory.value.id, false);
    }
    await searchMemories();
    if (path) {
      appSettings.value = await settingsGet();
      await readArchiveStatus();
      notify(migrate ? '记忆库已迁移' : '已打开记忆库');
    }
  } catch (cause) {
    error.value = String(cause);
  } finally {
    loading.value = false;
  }
}

async function refreshWorkspace() {
  loading.value = true;
  error.value = '';
  try {
    workspace.value = await workspaceRefresh();
    allMemories.value = await memorySearch(null, '');
    await invalidateGraph();
    if (activeRepositoryId.value && !repositories.value.some(repo => repo.id === activeRepositoryId.value)) activeRepositoryId.value = null;
    const currentId = selectedMemory.value?.id;
    if (currentId && !allMemories.value.some(memory => memory.id === currentId)) {
      selectedMemory.value = null;
      relatedHistory.value = [];
    }
    await searchMemories();
    if (currentId && allMemories.value.some(memory => memory.id === currentId)) await readMemory(currentId, false);
    notify('已重新读取本地 Markdown');
  } catch (cause) {
    error.value = String(cause);
  } finally {
    loading.value = false;
  }
}

async function saveAppSettings(input: SettingsInput) {
  savingSettings.value = true;
  error.value = '';
  try {
    const embeddingChanged = appSettings.value?.embedding.baseUrl !== input.embedding.baseUrl.trim() ||
      appSettings.value?.embedding.model !== input.embedding.model.trim() || input.embedding.apiKey !== undefined;
    appSettings.value = await settingsSave(input);
    if (embeddingChanged && semanticActive.value) await searchMemories();
    await updateArchive();
    notify('模型服务和归档设置已保存');
  } catch (cause) {
    error.value = String(cause);
  } finally {
    savingSettings.value = false;
  }
}

async function readArchiveStatus() {
  const root = workspace.value?.root;
  try {
    const next = await archiveStatus();
    if (workspace.value?.root !== root) return;
    archive.value = next;
    archiveReadError.value = '';
    return next;
  } catch (cause) {
    if (workspace.value?.root === root) archiveReadError.value = String(cause);
  }
}

async function updateArchive() {
  if (pollingArchive || document.hidden || transferPanel.value?.busy || changingWorkspace.value) return;
  pollingArchive = true;
  try {
    const next = await readArchiveStatus();
    if (!next) return;
    const revision = JSON.stringify([next.memoryWrites, next.pendingEmbeddings, next.processingEmbeddings, next.failedEmbeddings]);
    if (revision !== archiveRevision && workspace.value && view.value !== 'editor' && !loading.value && !saving.value) {
      archiveRevision = revision;
      const writtenChanged = lastMemoryWrites !== null && lastMemoryWrites !== next.memoryWrites;
      if (writtenChanged) {
        workspace.value = await workspaceRefresh();
      }
      lastMemoryWrites = next.memoryWrites;
      allMemories.value = await memorySearch(null, '');
      if (writtenChanged) await invalidateGraph();
      if (view.value === 'workbench' && !searching.value) await searchMemories();
      else workbenchNeedsRefresh = true;
      if (selectedMemory.value) await readMemory(selectedMemory.value.id, false);
    }
  } catch (cause) {
    error.value = String(cause);
  } finally {
    pollingArchive = false;
  }
}

async function retryArchive() {
  retryingArchive.value = true;
  error.value = '';
  try {
    await archiveRetry();
    await updateArchive();
    notify('已重新加入后台处理队列');
  } catch (cause) {
    error.value = String(cause);
  } finally {
    retryingArchive.value = false;
  }
}

function selectMemory(id: string) {
  graphReaderActive.value = false;
  relatedHistory.value = [];
  void readMemory(id);
}

async function openRelated(id: string) {
  if (selectedMemory.value) relatedHistory.value.push(selectedMemory.value.id);
  await readMemory(id);
}

function returnToPrevious() {
  const id = relatedHistory.value.pop();
  if (id) void readMemory(id);
  else {
    narrowReader.value = false;
    void nextTick(() => (resultScroll.value?.querySelector<HTMLButtonElement>('.memory-result.selected') ?? searchInput.value)?.focus({ preventScroll: true }));
  }
}

function startEditor(memory: MemoryRecord | null = null) {
  navigate(() => {
    if (!repositories.value.length) {
      repositoryDialog.value = true;
      return;
    }
    editingMemory.value = memory;
    editorKey.value++;
    error.value = '';
    view.value = 'editor';
  });
}

async function saveMemory(input: MemoryInput) {
  saving.value = true;
  error.value = '';
  try {
    const saved = await memorySave(input);
    allMemories.value = await memorySearch(null, '');
    if (workspace.value) for (const repo of workspace.value.repositories) repo.memoryCount = allMemories.value.filter(memory => memory.repositoryId === repo.id).length;
    await invalidateGraph();
    activeRepositoryId.value = saved.repositoryId;
    query.value = '';
    relatedHistory.value = [];
    selectedMemory.value = saved;
    view.value = 'workbench';
    narrowReader.value = true;
    await searchMemories();
    notify('记忆已保存到 Markdown');
  } catch (cause) {
    error.value = String(cause);
  } finally {
    saving.value = false;
  }
}

function selectDirectory() {
  if (loading.value || savingSettings.value) return;
  navigate(() => { void pickDirectory(true); });
}

function openDirectory() {
  if (loading.value || savingSettings.value) return;
  navigate(() => { void pickDirectory(false); });
}

async function pickDirectory(migrate: boolean) {
  try {
    const path = await chooseWorkspace(migrate);
    if (path) {
      changingWorkspace.value = true;
      await loadWorkspace(path, migrate);
    }
  } catch (cause) {
    error.value = String(cause);
  } finally {
    changingWorkspace.value = false;
  }
}

async function addRepository() {
  addingRepository.value = true;
  repositoryError.value = '';
  try {
    const repo = await repositoryAdd({ name: repositoryName.value.trim(), remote: repositoryRemote.value.trim() });
    workspace.value = await workspaceRefresh();
    activeRepositoryId.value = repo.id;
    query.value = '';
    relatedHistory.value = [];
    selectedMemory.value = null;
    view.value = 'workbench';
    repositoryDialog.value = false;
    repositoryName.value = '';
    repositoryRemote.value = '';
    await searchMemories();
    notify('仓库已接入，可以新增记忆');
  } catch (cause) {
    repositoryError.value = String(cause);
  } finally {
    addingRepository.value = false;
  }
}

async function openRepositoryMenu(event: MouseEvent | KeyboardEvent, repository: Repository) {
  repositoryMenuTrigger = event.currentTarget as HTMLElement;
  const rect = repositoryMenuTrigger.getBoundingClientRect();
  const x = 'clientX' in event ? event.clientX : rect.left + 20;
  const y = 'clientY' in event ? event.clientY : rect.bottom;
  repositoryMenu.value = { repository, x, y };
  await nextTick();
  const menu = repositoryMenuElement.value;
  if (menu && repositoryMenu.value) {
    repositoryMenu.value.x = Math.max(8, Math.min(x, window.innerWidth - menu.offsetWidth - 8));
    repositoryMenu.value.y = Math.max(8, Math.min(y, window.innerHeight - menu.offsetHeight - 8));
    menu.querySelector<HTMLButtonElement>('button')?.focus();
  }
}

function moveRepositoryMenuFocus(event: KeyboardEvent) {
  const buttons = Array.from(repositoryMenuElement.value?.querySelectorAll('button') ?? []);
  const current = buttons.indexOf(event.target as HTMLButtonElement);
  buttons[(current + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus();
}

function closeRepositoryMenu() {
  repositoryMenu.value = null;
}

function focusModal(element: Element) {
  element.querySelector<HTMLElement>('[data-initial-focus]')?.focus({ preventScroll: true });
}

function startRepositoryAction(kind: 'rename' | 'delete') {
  const repository = repositoryMenu.value?.repository;
  closeRepositoryMenu();
  if (!repository) return;
  navigate(() => {
    repositoryAction.value = { kind, repository };
    actionName.value = repository.name;
    actionError.value = '';
    void nextTick(() => { repositoryActionInput.value?.focus(); repositoryActionInput.value?.select(); });
  });
}

async function saveRepositoryAction() {
  const action = repositoryAction.value;
  if (!action) return;
  savingRepository.value = true;
  actionError.value = '';
  try {
    workspace.value = action.kind === 'rename'
      ? await repositoryRename(action.repository.id, actionName.value)
      : await repositoryDelete(action.repository.id);
    if (action.kind === 'delete') {
      if (activeRepositoryId.value === action.repository.id) activeRepositoryId.value = null;
      if (selectedMemory.value?.repositoryId === action.repository.id) {
        readVersion++;
        reading.value = false;
        selectedMemory.value = null;
        graphReaderActive.value = false;
        narrowReader.value = false;
      }
      relatedHistory.value = [];
      if (graphFocusId.value && allMemories.value.some(m => m.id === graphFocusId.value && m.repositoryId === action.repository.id)) graphFocusId.value = null;
    }
    allMemories.value = await memorySearch(null, '');
    await invalidateGraph();
    await searchMemories();
    if (selectedMemory.value) await readMemory(selectedMemory.value.id, false);
    repositoryAction.value = null;
    notify(action.kind === 'rename' ? '仓库已重命名' : '仓库已删除，记忆已移入本地回收目录');
  } catch (cause) {
    actionError.value = String(cause);
  } finally {
    savingRepository.value = false;
  }
}

async function openOriginal() {
  if (!selectedMemory.value) return;
  try { await openMemoryFile(selectedMemory.value.id); }
  catch (cause) { error.value = String(cause); }
}

function formatDate(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat('zh-CN', { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(date).replaceAll('/', '-');
}

function keydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && !event.defaultPrevented && !event.isComposing) {
    if (repositoryMenu.value) {
      closeRepositoryMenu();
      repositoryMenuTrigger?.focus({ preventScroll: true });
    }
    else if (repositoryAction.value && !savingRepository.value) repositoryAction.value = null;
    else if (repositoryDialog.value && !addingRepository.value) repositoryDialog.value = false;
    else if (pendingNavigation.value) pendingNavigation.value = null;
    else if (sidebarOpen.value) dismissSidebar();
  }
  if (!(event.metaKey || event.ctrlKey)) return;
  if (event.key.toLowerCase() === 'k') {
    event.preventDefault();
    showWorkbench();
    void nextTick(() => searchInput.value?.focus());
  }
  if (event.key.toLowerCase() === 's' && view.value === 'editor') {
    event.preventDefault();
    editor.value?.submit();
  }
  if (event.key.toLowerCase() === 's' && view.value === 'settings') {
    event.preventDefault();
    settingsPanel.value?.submit();
  }
}

watch(modalOpen, open => {
  if (open) {
    const active = document.activeElement as HTMLElement;
    modalTrigger = repositoryMenuElement.value?.contains(active) ? repositoryMenuTrigger : active;
  } else {
    void nextTick(() => {
      const target = modalTrigger?.isConnected && !modalTrigger.closest('[inert]') ? modalTrigger : sidebarToggle.value;
      target?.focus({ preventScroll: true });
      modalTrigger = undefined;
    });
  }
});

watch([activeRepositoryId, query, searchMode], () => {
  clearTimeout(searchTimer);
  if (resultScroll.value) resultScroll.value.scrollTop = 0;
  if (!workspace.value || loading.value) return;
  searchVersion++;
  searchError.value = '';
  semanticResult.value = null;
  searching.value = true;
  searchTimer = setTimeout(searchMemories, semanticActive.value ? 550 : 160);
}, { flush: 'sync' });

onMounted(async () => {
  stopSmoothWheelScroll = installSmoothWheelScroll();
  sidebarMedia.addEventListener('change', updateSidebarLayout);
  document.addEventListener('keydown', keydown);
  document.addEventListener('pointerdown', closeRepositoryMenu);
  window.addEventListener('resize', closeRepositoryMenu);
  document.addEventListener('scroll', closeRepositoryMenu, true);
  if (canUseNative) {
    unlistenClose = await getCurrentWindow().onCloseRequested(event => {
      if (discardOnClose) return;
      const dirty = view.value === 'editor' && editor.value?.dirty || view.value === 'settings' && settingsPanel.value?.dirty;
      if (!dirty && !saving.value && !savingSettings.value && !savingTheme.value && !savingRepository.value && !changingWorkspace.value && !transferPanel.value?.busy && !appSettings.value?.runInBackground) return;
      event.preventDefault();
      if (saving.value || savingSettings.value || savingTheme.value || savingRepository.value || changingWorkspace.value || transferPanel.value?.busy) return;
      const closeWindow = () => {
        if (appSettings.value?.runInBackground) {
          view.value = 'workbench';
          void getCurrentWindow().hide();
        } else {
          discardOnClose = true;
          void getCurrentWindow().close();
        }
      };
      if (dirty) pendingNavigation.value = closeWindow;
      else closeWindow();
    });
    await Promise.all([
      loadWorkspace(),
      themeGet().then(value => { theme.value = value; applyTheme(value); }).catch(cause => { error.value = String(cause); }),
    ]);
    try {
      appSettings.value = await settingsGet();
      await updateArchive();
    } catch (cause) {
      error.value = String(cause);
    }
    archiveTimer = setInterval(updateArchive, 3000);
  } else loading.value = false;
});
onBeforeUnmount(() => {
  clearTimeout(searchTimer);
  clearTimeout(statusTimer);
  clearInterval(archiveTimer);
  document.removeEventListener('keydown', keydown);
  sidebarMedia.removeEventListener('change', updateSidebarLayout);
  document.removeEventListener('pointerdown', closeRepositoryMenu);
  window.removeEventListener('resize', closeRepositoryMenu);
  document.removeEventListener('scroll', closeRepositoryMenu, true);
  stopSmoothWheelScroll?.();
  unlistenClose?.();
});
</script>

<template>
  <div class="app-shell" :class="{ 'sidebar-open': sidebarOpen, 'sidebar-collapsed': sidebarCollapsed }">
    <Transition name="scrim">
      <button v-if="sidebarOpen" class="sidebar-scrim" aria-label="收起导航" :inert="modalOpen" @click="dismissSidebar" />
    </Transition>
    <aside id="workspace-sidebar" class="sidebar glass-surface" :inert="!sidebarExpanded || modalOpen">
      <div class="brand">
        <span class="brand-mark">
          <img :src="brandMark" width="33" height="33" alt="" />
        </span>
        <div class="brand-name"><strong>知迹</strong><span>KnowTrace</span></div>
      </div>
      <div class="sidebar-section-label">工作空间</div>
      <nav class="primary-nav" aria-label="工作空间">
        <button :class="{ active: view === 'workbench' }" :aria-current="view === 'workbench' ? 'page' : undefined" @click="showWorkbench()">
          <BookOpen :size="18" />记忆工作台</button>
        <button :class="{ active: view === 'graph' }" :aria-current="view === 'graph' ? 'page' : undefined" :disabled="!workspace" data-testid="star-map-navigation" @click="showStarMap()">
          <Orbit :size="18" />记忆星图</button>
        <button :class="{ active: view === 'editor' }" :aria-current="view === 'editor' ? 'page' : undefined" :disabled="!workspace || saving" @click="startEditor()">
          <SquarePen :size="18" />新增记忆</button>
        <button :class="{ active: view === 'archive' }" :aria-current="view === 'archive' ? 'page' : undefined" @click="navigate(() => { view = 'archive'; })" data-testid="archive-navigation">
          <Activity :size="18" />归档与用量</button>
        <button :class="{ active: view === 'settings' }" :aria-current="view === 'settings' ? 'page' : undefined" @click="showSettingsSection('settings')">
          <Settings2 :size="18" />设置与接入</button>
      </nav>
      <div class="sidebar-divider" />
      <div class="sidebar-section-label repository-label">Git 仓库<button class="icon-button" title="接入仓库" aria-label="接入仓库" :disabled="!workspace" @click="navigate(() => { repositoryDialog = true; })">
          <Plus :size="16" />
        </button>
      </div>
      <nav class="repository-nav" aria-label="Git 仓库">
        <button :class="{ active: activeRepositoryId === null }" @click="showWorkbench(null)">
          <Database :size="14" />所有仓库<span>{{ totalMemories }}</span>
        </button>
        <button v-for="(repo, index) in repositories" :key="repo.id" :class="{ active: activeRepositoryId === repo.id }" :title="repo.remote" :data-repository-id="repo.id" aria-haspopup="menu" @click="showWorkbench(repo.id)" @contextmenu.prevent="openRepositoryMenu($event, repo)" @keydown.shift.f10.prevent="openRepositoryMenu($event, repo)">
          <i class="repo-dot" :style="{ background: index < 3 ? `var(--repo-${['a', 'b', 'c'][index]}, ${REPOSITORY_COLORS[index]})` : REPOSITORY_COLORS[index % REPOSITORY_COLORS.length] }" />
          <span class="repo-name">{{ repo.name }}</span>
          <ChevronRight v-if="activeRepositoryId === repo.id" :size="14" />
          <span v-else class="repo-count">{{ repo.memoryCount }}</span>
        </button>
      </nav>
      <button v-if="!repositories.length && workspace" class="add-repository" @click="repositoryDialog = true">
        <Plus :size="15" />接入第一个仓库</button>
      <ArchiveSidebarStatus v-if="canUseNative && (workspace || loading)" :state="archiveSidebarState" @navigate="openArchiveDestination" />
    </aside>
    <main class="main-shell" :inert="modalOpen">
      <header class="topbar glass-surface">
        <button ref="sidebarToggle" class="icon-button sidebar-toggle" aria-label="切换侧栏" aria-controls="workspace-sidebar" :aria-expanded="sidebarExpanded" @click="toggleSidebar">
          <PanelLeft :size="17" />
        </button>
        <span class="workspace-label">个人空间</span>
        <span class="breadcrumb-slash">/</span>
        <span :key="pageTitle" class="current-page-title">{{ pageTitle }}</span>
        <span class="storage-label">
          <Database :size="15" />本机存储</span>
      </header>
      <div v-if="error" class="error-banner" role="alert">
        <span>{{ error }}</span>
        <button class="icon-button" aria-label="关闭错误提示" @click="error = ''">
          <X :size="16" />
        </button>
      </div>
      <div v-if="workspace?.warnings.length" class="warnings-banner">
        <details>
          <summary>{{ workspace.warnings.length }} 个文件需要检查</summary>
          <p v-for="warning in workspace.warnings" :key="warning">{{ warning }}</p>
        </details>
      </div>
      <section v-if="!canUseNative" class="launch-view">
        <span class="welcome-mark">
          <BookOpen :size="34" />
        </span>
        <h1>在桌面端打开记忆工作台</h1>
        <code>pnpm desktop</code>
      </section>
      <section v-else-if="!workspace && view !== 'archive'" class="launch-view">
        <span class="welcome-mark">
          <BookOpen :size="34" />
        </span>
        <h1>{{ loading ? '正在打开记忆库…' : '打开本地记忆库' }}</h1>
        <button v-if="!loading" class="button primary" @click="openDirectory">
          <FolderOpen :size="17" />选择记忆库目录</button>
      </section>
      <div v-else-if="view === 'workbench'" class="workbench" :class="{ 'reader-visible': narrowReader }">
        <section class="memory-list glass-surface" aria-label="仓库记忆">
          <div class="list-head">
            <div class="list-heading-row">
              <h1>仓库记忆</h1>
              <button class="icon-button" title="重新读取本地文件" aria-label="刷新记忆库" :disabled="loading" @click="refreshWorkspace">
                <RefreshCw :size="17" :class="{ spinning: loading }" />
              </button>
            </div>
            <p class="repository-subtitle">
              <GitBranch :size="15" />
              <span>{{ activeRepository?.name ?? '所有仓库' }}</span>
            </p>
            <div class="search-field" :class="{ 'is-searching': searching }">
              <Search :size="19" />
              <input ref="searchInput" v-model="query" aria-label="搜索记忆" :placeholder="activeRepository ? '搜索当前仓库的记忆' : '搜索所有仓库的记忆'" data-testid="memory-search" @keydown.enter="submitSearch($event)" @keydown.up="moveSearchFocus" @keydown.down="moveSearchFocus" />
              <button v-if="query" class="icon-button" aria-label="清空搜索" @click="clearSearch">
                <X :size="15" />
              </button>
              <kbd v-else>⌘ K</kbd>
            </div>
            <div class="search-mode" role="group" aria-label="检索方式">
              <button :class="{ active: searchMode === 'semantic' }" :aria-pressed="searchMode === 'semantic'" data-testid="semantic-mode" @click="searchMode = 'semantic'">自动检索</button>
              <button :class="{ active: searchMode === 'keyword' }" :aria-pressed="searchMode === 'keyword'" data-testid="keyword-mode" @click="searchMode = 'keyword'">关键词</button>
            </div>
            <div class="list-result-label">
              <span>{{ query.trim() ? '搜索结果' : '最近记忆' }}<b>{{ results.length }}</b>
              </span>
              <span role="status">{{ searching ? '正在检索…' : searchError ? '检索未完成' : semanticResult ? `${searchLabel} · 前 5 条` : query.trim() ? '关键词匹配' : '按更新时间' }}</span>
            </div>
          </div>
          <div ref="resultScroll" class="result-scroll" :aria-busy="searching">
            <div v-if="searchError" class="search-error" role="alert" data-testid="search-error">
              <h2>{{ semanticActive ? '语义检索暂不可用' : '检索暂不可用' }}</h2>
              <p>{{ searchError }}</p>
              <button v-if="semanticActive" class="button" @click="useKeywordSearch">改用关键词检索</button>
              <button v-else class="button" @click="submitSearch()">重新检索</button>
            </div>
            <p v-if="semanticResult?.fallbackReason" class="search-index-note" role="status">已使用本地关键词：{{ semanticResult.fallbackReason }}</p>
            <p v-else-if="semanticResult?.pendingEmbeddings" class="search-index-note">{{ semanticResult.pendingEmbeddings }} 条向量未就绪，{{ semanticResult.searchMode === 'keyword' ? '当前使用本地关键词。' : '已用关键词补查。' }}</p>
            <div v-for="memory in results" :key="memory.id" class="memory-result-group">
            <button class="memory-result" :class="{ selected: selectedMemory?.id === memory.id }" :aria-current="selectedMemory?.id === memory.id || undefined" :data-memory-id="memory.id" @click="selectMemory(memory.id)" @keydown.up="moveSearchFocus" @keydown.down="moveSearchFocus">
              <span class="result-meta">
                <span>
                  <i />{{ memory.kind || '工作记录' }}</span>
                <time>{{ formatDate(memory.updatedAt) }}</time>
              </span>
              <h2>{{ memory.title }}</h2>
              <p v-if="memory.summary">{{ memory.summary }}</p>
              <span v-if="memory.matchesHistoryOnly" class="result-index-state">命中{{ memory.matchedSections?.filter(section => section.status !== 'current').map(section => section.title).join('、') }}，上方为当前结论</span>
              <span v-if="memory.embeddingStatus === 'pending' || memory.embeddingStatus === 'processing'" class="result-index-state"><Clock3 :size="11" />向量{{ memory.embeddingStatus === 'processing' ? '生成中' : '待生成' }}</span>
              <span v-else-if="memory.embeddingStatus === 'failed'" class="result-index-state failed">向量生成失败</span>
              <span v-if="!activeRepositoryId" class="result-repository">{{ repositories.find(repo => repo.id === memory.repositoryId)?.name }}</span>
            </button>
            <div v-if="memory.groupCount && memory.groupCount > 1" class="duplicate-group">
              <button class="text-button" :aria-expanded="expandedGroup === memory.id" @click="expandedGroup === memory.id ? expandedGroup = null : expandGroup(memory)">相同检索内容 · {{ memory.groupCount }} 条</button>
              <template v-if="expandedGroup === memory.id">
                <p v-if="groupError" role="alert">{{ groupError }}</p>
                <p v-if="groupLoading">正在读取同组记忆…</p>
                <template v-else-if="groupResult">
                  <button v-for="member in groupResult.results" :key="member.id" class="group-member text-button" @click="selectMemory(member.id)">{{ member.title }}<small>{{ member.id }}</small></button>
                  <div class="group-pagination">
                    <button class="text-button" :disabled="!groupResult.offset" @click="expandGroup(memory, Math.max(0, (groupResult.offset ?? 0) - 5))">上一页</button>
                    <span>{{ (groupResult.offset ?? 0) + 1 }}–{{ Math.min((groupResult.offset ?? 0) + 5, groupResult.total ?? 0) }} / {{ groupResult.total }}</span>
                    <button class="text-button" :disabled="!groupResult.hasMore" @click="expandGroup(memory, (groupResult.offset ?? 0) + 5)">下一页</button>
                  </div>
                </template>
              </template>
            </div>
            </div>
            <div v-if="!results.length && !searching && !searchError" class="list-empty">
              <Search v-if="query" :size="26" />
              <FolderOpen v-else :size="26" />
              <h2>{{ query ? '没有找到相关记忆' : '仓库记忆会出现在这里' }}</h2>
              <button v-if="query" class="button" @click="clearSearch">清空搜索</button>
              <button v-else class="button" @click="startEditor()">
                <Plus :size="15" />新增记忆</button>
            </div>
          </div>
          <footer class="list-footer search-footer">
            <div v-if="semanticResult" class="search-metrics" data-testid="search-metrics">
              <span>{{ semanticResult.queryVectorCached ? '复用查询向量' : semanticResult.embeddingLatencyMs ? `向量 API ${semanticResult.embeddingLatencyMs} ms` : searchLabel }} · 本地 {{ semanticResult.localSearchMs.toFixed(1) }} ms</span>
              <span>{{ semanticResult.queryVectorCached ? '本次 0 token' : semanticResult.queryUsageReported ? `本次 ${semanticResult.embeddingInputTokens} tokens` : '用量未返回' }}</span>
            </div>
            <div class="search-footer-status">
              <Check :size="15" />
              <button v-if="pendingVectors" class="index-queue-button" @click="navigate(() => { view = 'archive'; })">{{ pendingVectors }} 条向量待生成</button>
              <span v-else>{{ semanticActive ? '按需读取本地正文' : '本地关键词检索' }}</span>
              <span class="zero-token">{{ query.trim() ? '正文按需读取' : '不调用模型' }}</span>
            </div>
          </footer>
        </section>
        <section class="memory-reader" aria-label="记忆详情" :aria-busy="reading">
          <header class="reader-toolbar">
            <button class="reader-back" @click="returnToPrevious">
              <ArrowLeft :size="16" />返回{{ relatedHistory.length ? '上一条记忆' : '搜索结果' }}</button>
            <span class="reader-label">
              <BookOpen :size="16" />记忆详情</span>
            <div v-if="selectedMemory" class="reader-actions">
              <button class="icon-button" aria-label="在星图中查看记忆" title="在星图中查看记忆" data-testid="memory-open-star-map" @click="showStarMap(selectedMemory.id)">
                <Orbit :size="17" />
              </button>
              <button class="text-button original-file" @click="openOriginal">
                <FileText :size="15" />打开原文件</button>
              <button class="icon-button" aria-label="编辑记忆" title="编辑记忆" @click="startEditor(selectedMemory)">
                <SquarePen :size="17" />
              </button>
              <button class="icon-button" aria-label="查看更新日志" title="查看更新日志" data-testid="memory-open-update-log" @click="updateLogOpen = true">
                <History :size="17" />
              </button>
            </div>
          </header>
          <div v-if="selectedMemory" :key="selectedMemory.id" class="reader-scroll" :class="{ 'is-reading': reading }" data-testid="memory-detail">
            <button v-if="graphReaderActive" class="related-return" data-testid="memory-return-star-map" @click="showStarMap(null)">
              <ArrowLeft :size="15" />返回记忆星图
            </button>
            <button v-if="relatedHistory.length" class="related-return" @click="returnToPrevious">
              <ArrowLeft :size="15" />返回上一条记忆
            </button>
            <div class="memory-kicker">
              <span class="kind-chip">{{ selectedMemory.kind }}</span>
              <span class="memory-id">{{ selectedMemory.id }}</span>
            </div>
            <h1 ref="readerTitle" tabindex="-1">{{ selectedMemory.title }}</h1>
            <div class="memory-meta">
              <span>
                <GitBranch :size="15" />{{ currentRepository?.name }}</span>
              <span>
                <Clock3 :size="15" />{{ formatDate(selectedMemory.updatedAt) }}</span>
            </div>
            <div v-if="selectedMemory.tags.length" class="memory-tags">
              <span v-for="tag in selectedMemory.tags" :key="tag"># {{ tag }}</span>
            </div>
            <div v-if="selectedMemory.embeddingStatus === 'unconfigured'" class="memory-index-banner" role="status">
              <Clock3 :size="16" />
              <span>正文已保存，配置向量 API 后可生成检索索引。</span>
              <button @click="navigate(() => { view = 'settings'; })">配置向量 API</button>
            </div>
            <div v-else-if="selectedMemory.embeddingStatus === 'pending' || selectedMemory.embeddingStatus === 'processing'" class="memory-index-banner" role="status">
              <Clock3 :size="16" />
              <span>正文已保存，向量{{ selectedMemory.embeddingStatus === 'processing' ? '正在后台生成' : '等待后台生成' }}。</span>
              <button @click="openArchiveDestination('archive')">查看归档</button>
            </div>
            <div v-else-if="selectedMemory.embeddingStatus === 'failed'" class="memory-index-banner failed" role="status">
              <span>正文已保存，向量生成失败。{{ selectedMemory.embeddingError }}</span>
              <button @click="navigate(() => { view = 'archive'; })">查看处理队列</button>
            </div>
            <MemoryProvenance :memory="selectedMemory" :adopting="adopting" @adopt="adoptSuggestion" />
            <MarkdownView :markdown="selectedMemory.markdown" :title="selectedMemory.title" />
            <div v-if="selectedMemory.source" class="memory-source">
              <FileText :size="16" />
              <div>
                <span>来源</span>
                <p>{{ selectedMemory.source }}</p>
              </div>
            </div>
            <section v-if="relatedMemories.length" class="related-section">
              <h2>
                <Link2 :size="16" />关联记忆<span>{{ relatedMemories.length }}</span>
              </h2>
              <button v-for="item in relatedMemories" :key="item.id" class="related-link" :disabled="!item.memory" @click="openRelated(item.id)">
                <FileText :size="16" />
                <span>{{ item.memory?.title ?? item.id }}<small>{{ item.memory ? repositories.find(repo => repo.id === item.memory?.repositoryId)?.name : '关联条目暂未找到' }}</small>
                </span>
                <ArrowRight :size="16" />
              </button>
            </section>
            <div class="file-location" :title="selectedMemory.path">
              <FileText :size="13" />{{ selectedMemory.path }}</div>
          </div>
          <div v-else class="reader-empty">
            <span class="welcome-mark">
              <BookOpen :size="31" />
            </span>
            <h1>{{ totalMemories ? '选择一条记忆查看详情' : '建立你的记忆库' }}</h1>
            <button v-if="!totalMemories" class="button primary" @click="startEditor()">
              <Plus :size="16" />记录第一条记忆</button>
          </div>
        </section>
      </div>
      <MemoryEditor v-else-if="view === 'editor'" :key="editorKey" ref="editor" :memory="editingMemory" :repositories="repositories" :active-repository-id="activeRepositoryId" :all-memories="allMemories" :saving="saving" @save="saveMemory" @cancel="showWorkbench()" />
      <ArchiveUsagePage v-else-if="view === 'archive'" v-model="archiveTab" :status="archive" :archive-sources="appSettings?.archiveSources" :memories="allMemories" :read-error="archiveReadError" :retrying="retryingArchive" @retry="retryArchive" @open-memory="id => navigate(() => { view = 'workbench'; selectMemory(id); })" @settings="navigate(() => { view = 'settings'; })" />
      <SettingsPanel v-else-if="view === 'settings' && workspace" ref="settingsPanel" :settings="appSettings" :workspace="workspace" :saving="savingSettings" :loading="loading" :transferring="transferPanel?.busy" @save="saveAppSettings" @choose-workspace="selectDirectory" @open-workspace="openDirectory" @refresh="refreshWorkspace" @add-repository="navigate(() => { repositoryDialog = true; })" @repository-menu="openRepositoryMenu">
        <template #theme="{ back }"><ThemePanel embedded :theme="theme" :saving="savingTheme" @change="changeTheme" @back="back" /></template>
        <template #transfer="{ back }">
          <TransferPanel v-if="!savingTheme" ref="transferPanel" embedded :workspace="workspace" :settings="appSettings" :theme="theme" @imported="importedTransfer" @back="back" />
          <p v-else class="settings-empty" aria-busy="true">正在保存主题…</p>
        </template>
      </SettingsPanel>
      <section v-else-if="view === 'graph' && !graph" class="launch-view" :aria-busy="graphLoading">
        <Orbit :size="34" />
        <h1>{{ graphLoading ? '正在读取记忆星图…' : '读取星图未完成' }}</h1>
        <button v-if="!graphLoading" class="button" @click="loadGraph">重新读取</button>
      </section>
      <MemoryStarMap v-if="workspace && graph" v-show="view === 'graph'" ref="starMap" :active="view === 'graph'" :graph="graph" :repositories="repositories" :initial-memory-id="graphFocusId" @open-memory="openGraphMemory" />
    </main>
    <Transition name="notice">
      <div v-if="status" class="toast glass-raised" role="status"><Check :size="16" />{{ status }}</div>
    </Transition>
    <MemoryUpdateLog v-if="updateLogOpen && selectedMemory" :memory="selectedMemory" @close="updateLogOpen = false" />
    <Transition name="popover">
      <div v-if="repositoryMenu" ref="repositoryMenuElement" class="repository-menu glass-raised" role="menu" aria-label="仓库操作" :style="{ left: `${repositoryMenu.x}px`, top: `${repositoryMenu.y}px` }" @pointerdown.stop @contextmenu.prevent @keydown.down.prevent="moveRepositoryMenuFocus" @keydown.up.prevent="moveRepositoryMenuFocus">
        <button role="menuitem" @click="startRepositoryAction('rename')"><SquarePen :size="15" />重命名</button>
        <button role="menuitem" class="danger-text" @click="startRepositoryAction('delete')"><Trash2 :size="15" />删除仓库</button>
      </div>
    </Transition>
    <Transition name="overlay" @enter="focusModal">
      <div v-if="repositoryAction" class="modal-backdrop">
        <form class="modal glass-raised" :role="repositoryAction.kind === 'delete' ? 'alertdialog' : 'dialog'" aria-modal="true" aria-labelledby="repository-action-title" @submit.prevent="saveRepositoryAction">
          <div class="modal-heading"><h2 id="repository-action-title">{{ repositoryAction.kind === 'rename' ? '重命名仓库' : '删除仓库' }}</h2><button type="button" class="icon-button" aria-label="关闭仓库操作" :disabled="savingRepository" @click="repositoryAction = null"><X :size="18" /></button></div>
          <label v-if="repositoryAction.kind === 'rename'" class="field">仓库名称<input ref="repositoryActionInput" v-model="actionName" required :disabled="savingRepository" data-initial-focus /></label>
          <template v-else><p>删除「{{ repositoryAction.repository.name }}」及其 {{ repositoryAction.repository.memoryCount }} 条记忆？相关记忆的关联也会移除。</p><p>记忆文件将移入本地记忆库的 .aimemory/trash 目录。原始 Git 项目不受影响；后续新归档仍可重新接入此仓库。</p></template>
          <p v-if="actionError" class="inline-error" role="alert">{{ actionError }}</p>
          <footer><button type="button" class="button" :disabled="savingRepository" :data-initial-focus="repositoryAction.kind === 'delete' ? '' : undefined" @click="repositoryAction = null">取消</button><button class="button" :class="repositoryAction.kind === 'delete' ? 'danger-button' : 'primary'" :disabled="savingRepository">{{ savingRepository ? '正在处理…' : repositoryAction.kind === 'rename' ? '保存名称' : '删除仓库' }}</button></footer>
        </form>
      </div>
    </Transition>
    <Transition name="overlay" @enter="focusModal">
      <div v-if="pendingNavigation" class="modal-backdrop">
        <section class="modal glass-raised" role="alertdialog" aria-modal="true" aria-labelledby="discard-title">
          <h2 id="discard-title">有未保存的更改</h2>
          <p>离开此页面后，本次更改会被放弃。</p>
          <footer>
            <button class="button" data-initial-focus @click="pendingNavigation = null">继续编辑</button>
            <button class="button primary" @click="discardAndNavigate">放弃更改并离开</button>
          </footer>
        </section>
      </div>
    </Transition>
    <Transition name="overlay" @enter="focusModal">
      <div v-if="repositoryDialog" class="modal-backdrop">
        <form class="modal glass-raised" role="dialog" aria-modal="true" aria-labelledby="repository-title" @submit.prevent="addRepository">
          <div class="modal-heading">
            <h2 id="repository-title">接入 Git 仓库</h2>
            <button type="button" class="icon-button" aria-label="关闭接入仓库" :disabled="addingRepository" @click="repositoryDialog = false">
              <X :size="18" />
            </button>
          </div>
          <label class="field">仓库名称<input v-model="repositoryName" required data-initial-focus placeholder="frontend-web" data-testid="repository-name" />
          </label>
          <label class="field">Git 远程地址<input v-model="repositoryRemote" required placeholder="https://github.com/team/frontend-web.git" data-testid="repository-remote" />
          </label>
          <p v-if="repositoryError" class="inline-error" role="alert">{{ repositoryError }}</p>
          <footer>
            <button type="button" class="button" :disabled="addingRepository" @click="repositoryDialog = false">取消</button>
            <button class="button primary" :disabled="addingRepository">{{ addingRepository ? '正在接入…' : '接入仓库' }}</button>
          </footer>
        </form>
      </div>
    </Transition>
  </div>
</template>
