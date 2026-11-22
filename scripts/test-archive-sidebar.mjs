import assert from 'node:assert/strict';
import { getArchiveSidebarState } from '../src/archive-sidebar.ts';

const idle = {
  enabled: true,
  startedAt: '2026-11-21T08:30:00Z',
  scanning: false,
  processedTurns: 0,
  writtenMemories: 0,
  pendingEmbeddings: 0,
  failedEmbeddings: 0,
  processingEmbeddings: 0,
  chatInputTokens: 0,
  chatOutputTokens: 0,
  embeddingInputTokens: 0,
  recentTurns: [],
  lastError: null,
  usageIncomplete: false,
};
const paused = { ...idle, enabled: false };
const queued = { ...idle, pendingEmbeddings: 3, scanning: true };
const indexing = { ...queued, processingEmbeddings: 1 };
const extracting = { ...indexing, recentTurns: [{ status: 'pending' }, { status: 'extracting' }] };
const failed = { ...extracting, failedEmbeddings: 2 };

for (const [snapshot, label, tone, icon, target] of [
  [null, '正在读取状态', 'neutral', 'ellipsis', null],
  [{ ...paused, startedAt: null }, '尚未启用归档', 'neutral', 'power', 'settings'],
  [{ ...paused, pendingEmbeddings: 2, recentTurns: [{ status: 'pending' }] }, '已暂停归档', 'neutral', 'pause', 'settings'],
  [{ ...idle, recentTurns: [{ status: 'saved' }, { status: 'skipped' }] }, '等待新轮次', 'ready', 'radio', 'archive'],
  [{ ...idle, scanning: true }, '正在检查新轮次', 'working', 'scan-line', 'archive'],
  [queued, '3 条记忆待索引', 'neutral', 'layers', 'archive'],
  [{ ...queued, recentTurns: [{ status: 'pending' }] }, '轮次等待提取', 'neutral', 'clock-3', 'archive'],
  [{ ...indexing, enabled: false }, '正在生成索引', 'working', 'layers-2', 'archive'],
  [{ ...extracting, enabled: false }, '正在提取记忆', 'working', 'file-text', 'archive'],
  [{ ...failed, enabled: false }, '2 条记忆索引失败', 'failed', 'circle-alert', 'archive'],
  [{ ...idle, recentTurns: [{ status: 'skipped', error: '该轮工作目录没有 Git 远程仓库' }] }, '等待新轮次', 'ready', 'radio', 'archive'],
  [{ ...idle, failedEmbeddings: 1, recentTurns: [{ status: 'skipped' }] }, '1 条记忆索引失败', 'failed', 'circle-alert', 'archive'],
  [{ ...indexing, recentTurns: [{ status: 'failed' }] }, '归档需要检查', 'failed', 'circle-alert', 'archive'],
  [{ ...paused, lastError: '无法读取会话目录' }, '归档需要检查', 'failed', 'circle-alert', 'archive'],
]) {
  assert.deepEqual(getArchiveSidebarState(snapshot), { label, tone, icon, target });
}

for (const previous of [null, idle]) {
  assert.deepEqual(getArchiveSidebarState(previous, true), {
    label: '归档需要检查', tone: 'failed', icon: 'circle-alert', target: 'archive',
  });
}
console.log('归档侧栏的状态、并发优先级和跳转目标核对通过。');
