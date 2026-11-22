import type { ArchiveStatus } from './types';

export interface ArchiveSidebarState {
  label: string;
  tone: 'ready' | 'working' | 'neutral' | 'failed';
  icon: 'radio' | 'scan-line' | 'clock-3' | 'file-text' | 'layers' | 'layers-2' | 'circle-alert' | 'ellipsis' | 'power' | 'pause';
  target: 'archive' | 'settings' | null;
}

export function getArchiveSidebarState(status: ArchiveStatus | null, readFailed = false): ArchiveSidebarState {
  if (readFailed || status?.lastError || status?.recentTurns.some(turn => turn.status === 'failed')) {
    return { label: '归档需要检查', tone: 'failed', icon: 'circle-alert', target: 'archive' };
  }
  if (!status) {
    return { label: '正在读取状态', tone: 'neutral', icon: 'ellipsis', target: null };
  }
  if (status.failedEmbeddings > 0) {
    return { label: `${status.failedEmbeddings} 条记忆索引失败`, tone: 'failed', icon: 'circle-alert', target: 'archive' };
  }
  if (status.recentTurns.some(turn => turn.status === 'extracting')) {
    return { label: '正在提取记忆', tone: 'working', icon: 'file-text', target: 'archive' };
  }
  if (status.processingEmbeddings > 0) {
    return { label: '正在生成索引', tone: 'working', icon: 'layers-2', target: 'archive' };
  }
  if (!status.enabled) {
    return status.startedAt
      ? { label: '已暂停归档', tone: 'neutral', icon: 'pause', target: 'settings' }
      : { label: '尚未启用归档', tone: 'neutral', icon: 'power', target: 'settings' };
  }
  if (status.recentTurns.some(turn => turn.status === 'pending')) {
    return { label: '轮次等待提取', tone: 'neutral', icon: 'clock-3', target: 'archive' };
  }
  if (status.pendingEmbeddings > 0) {
    return { label: `${status.pendingEmbeddings} 条记忆待索引`, tone: 'neutral', icon: 'layers', target: 'archive' };
  }
  if (status.scanning) {
    return { label: '正在检查新轮次', tone: 'working', icon: 'scan-line', target: 'archive' };
  }
  return { label: '等待新轮次', tone: 'ready', icon: 'radio', target: 'archive' };
}
