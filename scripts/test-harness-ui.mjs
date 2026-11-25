import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks, stripTypeScriptTypes } from 'node:module';
import { extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { compileScript, parse } from 'vue/compiler-sfc';
import { createRenderer, h, nextTick, reactive } from 'vue';

// Compile the actual SFCs in memory; only the native IPC boundary is replaced.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith('.') && context.parentURL && !extname(specifier)) {
      const candidate = new URL(specifier + '.ts', context.parentURL);
      if (existsSync(candidate)) return nextResolve(candidate.href, context);
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url.endsWith('.vue')) {
      const filename = fileURLToPath(url);
      const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename });
      const { content } = compileScript(descriptor, { id: filename, inlineTemplate: true, templateOptions: { compilerOptions: { hoistStatic: false } } });
      return { format: 'module', source: stripTypeScriptTypes(content), shortCircuit: true };
    }
    if (url.endsWith('.ts')) return { format: 'module', source: stripTypeScriptTypes(readFileSync(new URL(url), 'utf8')), shortCircuit: true };
    if (/[.](svg|png)$/.test(url)) return { format: 'module', source: 'export default ' + JSON.stringify(url), shortCircuit: true };
    if (url === new URL('../package.json', import.meta.url).href) {
      const data = JSON.parse(readFileSync(new URL(url), 'utf8'));
      return { format: 'module', source: 'export const version = ' + JSON.stringify(data.version), shortCircuit: true };
    }
    return nextLoad(url, context);
  },
});

function element(tag, text = '') {
  return {
    tag, text, children: [], parent: null, props: {}, style: {}, checked: false,
    listeners: new Map(),
    getRootNode() { return globalThis.document; },
    addEventListener(name, handler) { this.listeners.set(name, [...(this.listeners.get(name) ?? []), handler]); },
    removeEventListener(name, handler) { this.listeners.set(name, (this.listeners.get(name) ?? []).filter(item => item !== handler)); },
    dispatch(name) { for (const handler of this.listeners.get(name) ?? []) handler({ target: this }); },
    showModal() { this.props.open = true; },
  };
}
const body = element('body');
const renderer = createRenderer({
  createElement: tag => element(tag), createText: text => element('#text', text), createComment: text => element('#comment', text),
  setText: (node, text) => { node.text = text; },
  setElementText: (node, text) => { node.text = text; node.children = []; },
  patchProp(node, key, previous, value) {
    node.props[key] = value;
    if (key === 'value') { node.value = value; node._value = value; }
    if (key === 'checked') node.checked = value;
    if (key === 'style' && value && typeof value === 'object') Object.assign(node.style, value);
  },
  insert(node, parent, anchor = null) {
    if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1);
    const index = anchor ? parent.children.indexOf(anchor) : -1;
    parent.children.splice(index < 0 ? parent.children.length : index, 0, node); node.parent = parent;
  },
  remove(node) { if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1); node.parent = null; },
  parentNode: node => node.parent,
  nextSibling: node => node.parent?.children[node.parent.children.indexOf(node) + 1] ?? null,
  querySelector: selector => selector === 'body' ? body : null,
  insertStaticContent(content, parent, anchor) {
    const node = element('#text', content.replace(/<[^>]+>/g, ''));
    const index = anchor ? parent.children.indexOf(anchor) : -1;
    parent.children.splice(index < 0 ? parent.children.length : index, 0, node); node.parent = parent;
    return [node, node];
  },
});

const codexSummary = { records: 1, sessions: 1, input: 100, cached: 20, cacheWrite: 0, output: 10, reasoning: 0, cacheKnownInput: 100, reasoningKnownOutput: 10, pricedTokens: 110, pricedRecords: 1, cost: .001 };
const harnessSummary = { ...codexSummary, input: 50, cached: 0, output: 10, cacheKnownInput: 0, reasoningKnownOutput: 0, pricedTokens: 0, pricedRecords: 0, cost: 0 };
const emptySummary = Object.fromEntries(Object.keys(codexSummary).map(key => [key, 0]));
const at = Date.now();
const sessionRows = [
  { id: 'shared-session', title: 'Codex 旧会话', parentId: null, role: 'main', model: 'same-model', project: '/repo', firstAt: at, lastAt: at, summary: codexSummary },
  { client: 'harness', id: 'harness:shared-session', title: 'Harness 会话', parentId: 'harness:parent-session', role: 'agent', model: 'same-model', project: '/repo', firstAt: at, lastAt: at, summary: harnessSummary },
];
const calls = [];
let nextDirectory = '/chosen/harness';
let exportPath = '/exports/usage.csv';
let legacyOverview = false;
let deferCalendar = false;
const pendingCalendars = [];
const selectedRows = (client, project = null) => sessionRows.filter(row => (!client || (row.client ?? 'codex') === client) && (project === null || row.project === project));
const summarize = rows => rows.reduce((sum, row) => Object.fromEntries(Object.keys(sum).map(key => [key, sum[key] + row.summary[key]])), { ...emptySummary });
function overview(filters) {
  const rows = selectedRows(filters.client, filters.project);
  const summary = summarize(rows);
  const projects = [...new Set(rows.map(row => row.project))].map(key => ({ key, summary: summarize(rows.filter(row => row.project === key)) }));
  return {
    summary, models: [{ key: 'same-model', summary }], projects, roles: ['main', 'agent'], availableModels: ['same-model'], availableProjects: [...new Set(selectedRows(filters.client).map(row => row.project))],
    trend: filters.buckets.map((bucket, index) => ({ ...bucket, summary: index ? { ...emptySummary } : summary })),
    quota: filters.client === 'harness' ? null : { at, threadId: 'shared-session', data: { primary: { used_percent: 10, window_minutes: 300, resets_at: null }, secondary: null } },
    issueCount: rows.length, status: { scanning: false, completed: 2, total: 2, lastSyncedAt: at, errors: [] }, preferences: { autoScan: false, intervalSeconds: 10 }, directory: '/legacy/codex',
    ...(legacyOverview ? {} : { directories: { codex: '/configured/codex', harness: '/configured/harness' } }),
  };
}
async function invoke(command, args = {}) {
  calls.push({ command, args: JSON.parse(JSON.stringify(args)) });
  if (command === 'usage_overview') {
    const result = overview(args.filters);
    if (deferCalendar && args.filters.buckets.length > 60) return new Promise(resolve => pendingCalendars.push({ client: args.filters.client, resolve: () => resolve(result) }));
    return result;
  }
  if (command === 'usage_sessions') { const items = selectedRows(args.filters.client, args.filters.project); return { items, total: items.length }; }
  if (command === 'usage_detail') {
    const parent = args.threadId === 'harness:parent-session';
    const session = parent ? { ...sessionRows[1], id: args.threadId, title: 'Harness 父会话', parentId: null } : sessionRows.find(row => row.id === args.threadId);
    assert.ok(session, 'namespaced thread ID must reach the native boundary unchanged');
    const record = { client: session.client, id: session.id + ':record', threadId: session.id, turnId: 'turn-1', at, model: session.model, provider: 'custom-endpoint', tier: '', project: session.project, role: session.role, tokens: { input: session.summary.input, cached: null, cacheWrite: 0, output: 10, reasoning: null }, cost: null, file: '/logs/' + session.id + '.jsonl', line: 2, basis: 'response' };
    return { session, records: [record], totalRecords: 1, costParts: [0, 0, 0, 0], costGroups: [], memories: [] };
  }
  if (command === 'usage_issues') return selectedRows(args.client).map(row => ({ client: row.client, file: '/logs/' + row.id, line: 2, reason: '缺少用量依据' }));
  if (command === 'usage_prices_get') return [
    { model: 'same-model', provider: 'custom-endpoint', tier: '', input: null, cached: null, cacheWrite: null, output: null, source: '暂无已核实的官方参考价', custom: false },
    { model: 'gpt-6.1-sol', provider: 'custom-endpoint', tier: '', input: 2, cached: 0.1, cacheWrite: null, output: 10, source: 'OpenAI 官方参考 · 2026-11-25', custom: false },
  ];
  if (command === 'plugin:dialog|open') return nextDirectory;
  if (command === 'plugin:dialog|save') return exportPath;
  if (command === 'mcp_http_status') return { running: false, url: 'http://127.0.0.1:0/mcp' };
  if (command === 'mcp_info') return { dataDirectory: '/data', vaultRoot: '/vault' };
  if (['usage_export', 'usage_open_source', 'usage_prices_save'].includes(command)) return;
  throw new Error('Unexpected native command: ' + command);
}
globalThis.window = { __TAURI_INTERNALS__: { invoke }, addEventListener() {}, removeEventListener() {} };
globalThis.Document = class Document {};
globalThis.ShadowRoot = class ShadowRoot {};
globalThis.document = Object.assign(new Document(), { hidden: false, body, addEventListener() {}, removeEventListener() {} });
globalThis.ResizeObserver = class { constructor(callback) { this.callback = callback; } observe() { this.callback([{ contentRect: { width: 700 } }]); } disconnect() {} };

function* nodes(node) { yield node; for (const child of node.children) yield* nodes(child); }
const text = node => (node.tag === '#comment' ? '' : node.text) + node.children.map(text).join('');
const byId = (root, id) => [...nodes(root)].find(node => node.props['data-testid'] === id);
const byClass = (root, name) => [...nodes(root)].find(node => String(node.props.class ?? '').split(' ').includes(name));
function clickNode(node) { assert.ok(node, 'click target exists'); node.props.onClick({ target: node, currentTarget: node, preventDefault() {}, stopPropagation() {} }); }
function click(root, label) { clickNode([...nodes(root)].find(node => node.tag === 'button' && text(node).trim() === label)); }
async function settle() { await nextTick(); await new Promise(setImmediate); await nextTick(); }
function check(root, id, checked) { const node = byId(root, id); assert.ok(node, id); node.checked = checked; node.dispatch('change'); }
function* vnodes(vnode) {
  if (!vnode || typeof vnode !== 'object') return;
  yield vnode;
  if (vnode.component) yield* vnodes(vnode.component.subTree);
  else if (Array.isArray(vnode.children)) for (const child of vnode.children) yield* vnodes(child);
}
const apps = new Set();
function mount(component, input) {
  const props = reactive(input); const root = element('root');
  const app = renderer.createApp({ render: () => h(component, props) });
  app.config.errorHandler = error => { console.error(error); throw error; };
  apps.add(app); app.mount(root);
  return { props, root, get vnode() { return app._instance.subTree; }, close() { app.unmount(); apps.delete(app); } };
}
async function source(mounted, value) {
  const select = [...vnodes(mounted.vnode)].find(vnode => vnode.type.__name === 'AppSelect' && vnode.props['data-testid'] === 'usage-client');
  assert.ok(select, 'source uses the existing AppSelect component');
  select.props['onUpdate:modelValue'](value); await settle();
}
function closeDrawer() { clickNode([...nodes(body)].find(node => node.tag === 'button' && node.props['aria-label'] === '关闭面板')); }
const lastCall = command => calls.filter(call => call.command === command).at(-1);
const { default: SettingsPanel } = await import('../src/components/SettingsPanel.vue');
const { default: ArchiveUsagePage } = await import('../src/components/ArchiveUsagePage.vue');
const { default: ArchiveSidebarStatus } = await import('../src/components/ArchiveSidebarStatus.vue');
const { default: UsagePanel } = await import('../src/components/UsagePanel.vue');
const { getArchiveSidebarState } = await import('../src/archive-sidebar.ts');
const usage = await import('../src/usage.ts');
const api = await import('../src/api.ts');

try {
  const service = { baseUrl: 'https://example.invalid/v1', model: 'same-model', hasApiKey: true };
  const legacySettings = { chat: service, embedding: service, codexDirectory: '/old/codex', archiveEnabled: true, archiveStartedAt: null, runInBackground: false };
  const saves = [];
  const settings = mount(SettingsPanel, { settings: legacySettings, workspace: { root: '/vault', repositories: [], warnings: [] }, saving: false, loading: false, onSave: input => saves.push(input) });
  await settle();
  const saveButton = byId(settings.root, 'save-settings');
  for (const tab of ['general', 'models', 'library', 'archive', 'mcp', 'theme', 'transfer', 'update']) {
    clickNode(byId(settings.root, 'settings-tab-' + tab)); await settle();
    assert.equal(byId(settings.root, 'save-settings'), saveButton, 'switching settings must retain the save slot');
    const hidden = !['models', 'archive'].includes(tab);
    assert.equal(saveButton.props['aria-hidden'], hidden);
    assert.equal(!!saveButton.props.disabled, hidden, 'a reserved hidden action must not be interactive');
  }
  clickNode(byId(settings.root, 'settings-tab-archive')); await settle();
  assert.doesNotMatch(text(settings.root), /选择需要自动归档的来源，下面的总开关同时控制所选来源。/);
  assert.equal(byId(settings.root, 'archive-source-codex').checked, true);
  assert.equal(byId(settings.root, 'archive-source-harness').checked, false);
  assert.equal(settings.vnode.component.exposed.dirty.value, false);
  const harnessDirectory = [...nodes(settings.root)].find(node => String(node.props.class).includes('settings-directory') && text(node).includes('DeepSeek Harness 数据目录'));
  clickNode([...nodes(harnessDirectory)].find(node => node.tag === 'button')); await settle();
  assert.match(lastCall('plugin:dialog|open').args.options.title, /DeepSeek Harness/);
  assert.equal(lastCall('plugin:dialog|open').args.options.directory, true);
  assert.match(text(byId(settings.root, 'harness-directory')), /chosen[/]harness/);
  assert.equal(text(byId(settings.root, 'codex-directory')), '/old/codex');
  check(settings.root, 'archive-source-harness', true); check(settings.root, 'archive-enabled', false); await settle();
  clickNode(byId(settings.root, 'save-settings')); await settle();
  assert.deepEqual(saves.at(-1).archiveSources, ['codex', 'harness']);
  assert.equal(saves.at(-1).harnessDirectory, '/chosen/harness');
  assert.equal(saves.at(-1).codexDirectory, '/old/codex');
  assert.equal(saves.at(-1).archiveEnabled, false);
  assert.equal(legacySettings.archiveSources, undefined, 'the draft must not mutate incoming settings');
  check(settings.root, 'archive-source-codex', false); await settle(); clickNode(byId(settings.root, 'save-settings'));
  assert.deepEqual(saves.at(-1).archiveSources, ['harness']);
  check(settings.root, 'archive-source-harness', false); await settle(); clickNode(byId(settings.root, 'save-settings'));
  assert.deepEqual(saves.at(-1).archiveSources, [], 'an explicit empty selection is not an old-settings default');
  nextDirectory = null; clickNode([...nodes(harnessDirectory)].find(node => node.tag === 'button')); await settle();
  assert.equal(text(byId(settings.root, 'harness-directory')), '/chosen/harness');
  settings.vnode.component.exposed.reset(); await settle();
  assert.equal(byId(settings.root, 'archive-source-codex').checked, true);
  assert.equal(byId(settings.root, 'archive-source-harness').checked, false);
  assert.equal(settings.vnode.component.exposed.dirty.value, false);
  await api.chooseCodexDirectory(); assert.match(lastCall('plugin:dialog|open').args.options.title, /Codex/);
  settings.close();

  const baseTurn = { repository: 'repo', sessionId: 'shared-session', turnId: 'turn-1', completedAt: new Date(at).toISOString(), status: 'saved', memoryCount: 1, chatInputTokens: 10, chatOutputTokens: 5, embeddingInputTokens: 0, chatLatencyMs: null, embeddingLatencyMs: null, error: null, usageIncomplete: false };
  const status = { enabled: true, startedAt: null, scanning: false, processedTurns: 2, writtenMemories: 2, memoryWrites: 2, pendingEmbeddings: 0, failedEmbeddings: 0, failedEmbeddingDetails: [], processingEmbeddings: 0, chatInputTokens: 20, chatOutputTokens: 10, embeddingInputTokens: 0, recentTurns: [{ ...baseTurn, sourceId: 'shared-session:turn-1' }, { ...baseTurn, client: 'harness', sourceId: 'harness:shared-session:turn-1' }], lastError: null, usageIncomplete: false };
  const archive = mount(ArchiveUsagePage, { modelValue: 'archive', status, memories: [], readError: '', retrying: false, archiveSources: ['codex', 'harness'] });
  await settle();
  assert.equal(text(byId(archive.root, 'session-usage-tab')), '会话用量');
  assert.match(text(byId(archive.root, 'archive-sources')), /Codex、DeepSeek Harness/);
  const turns = [...nodes(archive.root)].filter(node => node.props['data-source-id']);
  assert.deepEqual(turns.map(node => node.props['data-source-id']), ['shared-session:turn-1', 'harness:shared-session:turn-1']);
  assert.deepEqual(turns.map(node => node.props['data-client']), ['codex', 'harness']);
  assert.match(text(turns[0]), /Codex/); assert.match(text(turns[1]), /DeepSeek Harness/);
  archive.props.archiveSources = []; await settle(); assert.match(text(byId(archive.root, 'archive-sources')), /未选择来源/);
  archive.props.archiveSources = undefined; await settle(); assert.equal(text(byId(archive.root, 'archive-sources')), '归档来源：Codex');
  const sidebar = mount(ArchiveSidebarStatus, { state: getArchiveSidebarState(status) });
  assert.equal(text(byClass(sidebar.root, 'status-caption')), '自动归档');
  assert.equal(text(byClass(sidebar.root, 'status-label')), '等待新轮次');
  sidebar.close(); archive.close();

  const panel = mount(UsagePanel, {}); await settle();
  assert.equal(lastCall('usage_overview').args.filters.client, null);
  assert.equal(lastCall('usage_sessions').args.filters.client, null);
  assert.equal([...nodes(panel.root)].filter(node => node.props['data-usage-thread']).length, 2);
  assert.match(text(panel.root), /Codex 额度快照/);
  await source(panel, 'harness');
  assert.equal(lastCall('usage_overview').args.filters.client, 'harness');
  assert.equal(lastCall('usage_sessions').args.filters.client, 'harness');
  assert.equal(byId(panel.root, 'usage-panel').props['data-client'], 'harness');
  const harnessRows = [...nodes(panel.root)].filter(node => node.props['data-usage-thread']);
  assert.equal(harnessRows.length, 1); assert.equal(harnessRows[0].props['data-usage-thread'], 'harness:shared-session');
  assert.match(text(harnessRows[0]), /DeepSeek Harness/);
  assert.match(text(byId(panel.root, 'usage-metrics')), /未定价/);
  assert.doesNotMatch(text(panel.root), /Codex 额度快照|Codex 官方用量/);
  click(panel.root, '导出'); await settle();
  assert.equal(lastCall('usage_export').args.filters.client, 'harness');
  assert.equal(lastCall('plugin:dialog|save').args.options.defaultPath, 'harness-usage.csv');
  assert.match(lastCall('plugin:dialog|save').args.options.title, /DeepSeek Harness/);
  click(panel.root, 'Harness 会话'); await settle();
  assert.match(text(body), /DeepSeek Harness · 会话详情/);
  assert.match(text(body), /未定价/);
  click(body, '来源信息'); await settle(); assert.match(text(body), /来源客户端DeepSeek Harness/);
  assert.match(text(body), /harness:shared-session/);
  click(body, '查看父会话'); await settle();
  assert.equal(lastCall('usage_detail').args.threadId, 'harness:parent-session');
  assert.equal(lastCall('usage_detail').args.filters.client, 'harness');
  assert.equal(lastCall('usage_detail').args.filters.start, 0);
  click(body, '打开本地日志'); await settle(); assert.equal(lastCall('usage_open_source').args.recordId, 'harness:parent-session:record');
  closeDrawer(); await settle();
  click(panel.root, '数据来源'); await settle();
  assert.match(text(body), /Codex 会话日志/); assert.match(text(body), /DeepSeek Harness 会话日志/);
  assert.match(text(body), /configured[/]codex/); assert.match(text(body), /configured[/]harness/);
  const harnessScope = text(byId(body, 'harness-usage-scope'));
  assert.match(harnessScope, /原生会话的模型步骤用量，包含失败与重试，排除继承消费/);
  assert.match(harnessScope, /不包含压缩、标题等辅助请求/);
  assert.match(harnessScope, /不等同服务商账单/);
  assert.match(text(body), /与自动归档的来源选择独立/); closeDrawer(); await settle();
  clickNode(byClass(panel.root, 'usage-issue-link')); await settle();
  assert.equal(lastCall('usage_issues').args.client, 'harness'); assert.match(text(body), /DeepSeek Harness · 缺少用量依据/);
  closeDrawer(); await settle();
  click(panel.root, '定价'); await settle();
  const officialInput = () => [...nodes(body)].find(node => node.tag === 'input' && node.props['aria-label'] === 'gpt-6.1-sol custom-endpoint 默认档位 输入 单价');
  assert.equal(officialInput().value, '2', 'official prices prefill custom routing providers');
  assert.match(text(body), /官方 API 参考价/);
  click(body, '保存参考价格'); await settle();
  for (const key of ['input', 'cached', 'cacheWrite', 'output']) assert.equal(lastCall('usage_prices_save').args.prices[0][key], null, 'blank prices stay null, not zero');
  assert.deepEqual([lastCall('usage_prices_save').args.prices[1].input, lastCall('usage_prices_save').args.prices[1].cached, lastCall('usage_prices_save').args.prices[1].output], [2, 0.1, 10]);
  assert.equal(lastCall('usage_prices_save').args.prices[1].custom, false, 'saving untouched presets must not pin them as manual');
  const editOfficial = async value => { const input = officialInput(); input.value = value; input.dispatch('input'); input.props.onInput({ target: input }); await settle(); };
  click(panel.root, '定价'); await settle(); await editOfficial('0');
  click(body, '保存参考价格'); await settle();
  assert.equal(lastCall('usage_prices_save').args.prices[1].input, 0);
  assert.equal(lastCall('usage_prices_save').args.prices[1].custom, true);
  assert.equal(lastCall('usage_prices_save').args.prices[1].provider, 'custom-endpoint');
  click(panel.root, '定价'); await settle(); await editOfficial('7'); click(body, '恢复官方预设'); await settle();
  assert.ok(lastCall('usage_prices_save').args.prices.every(price => !price.custom && ['input', 'cached', 'cacheWrite', 'output'].every(key => price[key] === null)));
  assert.equal(officialInput().value, '2', 'restore reloads the official preset');
  closeDrawer(); await settle();
  click(panel.root, '使用日历'); await settle();
  assert.equal(lastCall('usage_overview').args.filters.client, 'harness');
  assert.ok(lastCall('usage_overview').args.filters.buckets.length > 60);
  await source(panel, 'codex');
  assert.equal(lastCall('usage_sessions').args.filters.client, 'codex');
  assert.match(text(panel.root), /Codex 额度快照/);
  deferCalendar = true;
  await source(panel, 'harness'); await source(panel, 'codex');
  assert.deepEqual(pendingCalendars.map(item => item.client), ['harness', 'codex']);
  pendingCalendars[1].resolve(); await settle(); pendingCalendars[0].resolve(); await settle(); deferCalendar = false;
  const days = [...nodes(panel.root)].filter(node => node.props['aria-label']?.endsWith(' Tokens'));
  assert.ok(days.some(node => node.props['aria-label'].endsWith('110 Tokens')), 'late Harness calendar response must not replace Codex data');
  assert.ok(!days.some(node => node.props['aria-label'].endsWith('60 Tokens')));
  legacyOverview = true; await source(panel, '');
  click(panel.root, '数据来源'); await settle();
  assert.match(text(body), /legacy[/]codex/); assert.match(text(body), /尚未配置目录/);
  closeDrawer(); await settle();
  exportPath = null; const previousExports = calls.filter(call => call.command === 'usage_export').length;
  click(panel.root, '导出'); await settle(); assert.equal(calls.filter(call => call.command === 'usage_export').length, previousExports);
  panel.close();

  legacyOverview = false; exportPath = '/exports/unknown-project.csv';
  sessionRows.push({ ...sessionRows[1], id: 'harness:unknown-project', title: 'Harness 未识别项目会话', project: '', parentId: null });
  const projectsPanel = mount(UsagePanel, {}); await settle(); await source(projectsPanel, 'harness');
  const projectSelect = () => [...vnodes(projectsPanel.vnode)].find(vnode => vnode.type.__name === 'AppSelect' && vnode.props['data-testid'] === 'usage-project');
  const unknownProject = projectSelect().props.options.find(option => option.label === '未识别项目');
  assert.ok(unknownProject && unknownProject.value !== '', 'unknown project has its own UI-only selection value');
  assert.equal(new Set(projectSelect().props.options.map(option => option.value)).size, projectSelect().props.options.length, 'all and unknown project options must not have duplicate values');
  projectSelect().props['onUpdate:modelValue'](unknownProject.value); await settle();
  for (const command of ['usage_overview', 'usage_sessions']) {
    assert.equal(lastCall(command).args.filters.project, '', 'unknown project is sent as an empty string, not null');
    assert.equal(lastCall(command).args.filters.client, 'harness');
  }
  assert.equal(text(byId(projectsPanel.root, 'usage-project')), '未识别项目');
  assert.equal(byId(projectsPanel.root, 'usage-project').props.title, '未识别项目');
  assert.deepEqual([...nodes(projectsPanel.root)].filter(node => node.props['data-usage-thread']).map(node => node.props['data-usage-thread']), ['harness:unknown-project']);
  click(projectsPanel.root, '使用日历'); await settle();
  assert.ok(lastCall('usage_overview').args.filters.buckets.length > 60);
  assert.equal(lastCall('usage_overview').args.filters.project, '');
  assert.equal(lastCall('usage_overview').args.filters.client, 'harness');
  click(projectsPanel.root, '导出'); await settle();
  assert.equal(lastCall('usage_export').args.filters.project, '');
  assert.equal(lastCall('usage_export').args.filters.client, 'harness');
  await source(projectsPanel, 'codex');
  assert.equal(lastCall('usage_sessions').args.filters.project, '');
  assert.equal(lastCall('usage_sessions').args.filters.client, 'codex');
  assert.equal([...nodes(projectsPanel.root)].filter(node => node.props['data-usage-thread']).length, 0, 'an unrecognized Harness cwd must not become the Codex/current process project');
  assert.equal(text(byId(projectsPanel.root, 'usage-project')), '未识别项目', 'selected label stays readable even when this client has no unknown projects');
  assert.equal(byId(projectsPanel.root, 'usage-project').props.title, '未识别项目');
  await source(projectsPanel, 'harness');
  projectSelect().props['onUpdate:modelValue'](''); await settle();
  for (const command of ['usage_overview', 'usage_sessions']) {
    assert.equal(lastCall(command).args.filters.project, null, 'clearing the project selection restores all projects');
    assert.equal(lastCall(command).args.filters.client, 'harness');
  }
  assert.equal(text(byId(projectsPanel.root, 'usage-project')), '全部项目');
  click(projectsPanel.root, '导出'); await settle();
  assert.equal(lastCall('usage_export').args.filters.project, null);
  assert.equal(lastCall('usage_export').args.filters.client, 'harness');
  click(projectsPanel.root, '项目汇总'); await settle(); click(projectsPanel.root, '未识别项目'); await settle();
  assert.equal(lastCall('usage_sessions').args.filters.project, '', 'clicking the unknown project group uses the same selection as the dropdown');
  assert.equal(lastCall('usage_sessions').args.filters.client, 'harness');
  assert.equal(text(byId(projectsPanel.root, 'usage-project')), '未识别项目');
  const projectDay = [...nodes(projectsPanel.root)].find(node => node.props['data-level'] > 0 && node.props['aria-label']?.endsWith(' Tokens'));
  clickNode(projectDay); await settle();
  assert.equal(lastCall('usage_sessions').args.filters.project, '');
  assert.equal(lastCall('usage_sessions').args.filters.client, 'harness');
  click(projectsPanel.root, '导出'); await settle();
  assert.equal(lastCall('usage_export').args.filters.project, '');
  assert.equal(lastCall('usage_export').args.filters.client, 'harness');
  assert.doesNotMatch(text(projectsPanel.root), /__unidentified/);
  assert.ok(calls.filter(call => ['usage_overview', 'usage_sessions', 'usage_export'].includes(call.command)).every(call => [null, '', '/repo'].includes(call.args.filters.project)), 'no internal project sentinel reaches IPC');
  projectsPanel.close(); sessionRows.pop();

  assert.equal(usage.clientLabel(), 'Codex'); assert.equal(usage.clientLabel('harness'), 'DeepSeek Harness');
  assert.equal(usage.money(null), '未定价'); assert.equal(usage.referenceCost(harnessSummary), '未定价');
  assert.equal(usage.referenceCost(emptySummary), '—'); assert.equal(usage.totalTokens(harnessSummary), 60);
  await usage.usageIssues(); assert.equal(lastCall('usage_issues').args.client, null);
  console.log('Harness 前端回归通过：旧设置、双目录与来源保存、归档来源、用量筛选/详情/父链/日历并发、双来源抽屉、空项目筛选、导出、官方价格预设/手动覆盖/恢复与未定价。');
} finally {
  for (const app of apps) app.unmount();
}
