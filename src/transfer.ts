import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import type { AppSettings, Workspace } from './types';
import type { ThemeConfig } from './theme';

export type TransferKind = 'memories' | 'config' | 'theme';

export interface MemoryPreviewRow {
  id: string;
  title: string;
  repository: string;
  status: 'new' | 'existing';
  markdown: string;
  localMarkdown: string | null;
  localRepository: string | null;
}

export interface LinkPreviewRow {
  from: string;
  to: string;
  status: 'new' | 'existing' | 'invalid';
}

export interface ConfigPreviewRow {
  label: string;
  before: string;
  after: string;
  changed: boolean;
}

export interface ImportPreview {
  token: string;
  kind: TransferKind;
  path: string;
  memories: MemoryPreviewRow[];
  links: LinkPreviewRow[];
  rows: ConfigPreviewRow[];
  addedMemories: number;
  existingMemories: number;
  newLinks: number;
  invalidLinks: number;
  changedCount: number;
  hasTheme: boolean;
  theme: ThemeConfig | null;
}

export interface TransferApplyResult {
  kind: TransferKind;
  addedMemories: number;
  existingMemories: number;
  newLinks: number;
  invalidLinks: number;
  settings: AppSettings | null;
  theme: ThemeConfig | null;
  workspace: Workspace | null;
}

const names = { memories: '记忆', config: '配置', theme: '主题' };
const filenames = { memories: 'knowtrace-memories.json', config: 'knowtrace-config.json', theme: 'knowtrace-theme.json' };
const filters = [{ name: 'KnowTrace JSON', extensions: ['json'] }];

export function transferPreview(kind: TransferKind, path: string): Promise<ImportPreview> {
  return invoke('transfer_preview', { kind, path });
}

export function transferApply(token: string): Promise<TransferApplyResult> {
  return invoke('transfer_apply', { token });
}

export function transferDiscard(token: string): Promise<void> {
  return invoke('transfer_discard', { token });
}

export async function exportTransfer(kind: 'memories' | 'config'): Promise<string | null> {
  const path = await save({ title: `导出${names[kind]}`, defaultPath: filenames[kind], filters });
  return path ? invoke('transfer_export', { kind, path }) : null;
}

export async function chooseImport(kind: 'memories' | 'config'): Promise<ImportPreview | null> {
  const path = await open({ title: `选择知迹${names[kind]}包`, multiple: false, directory: false, filters });
  return path ? transferPreview(kind, path) : null;
}

export async function exportTheme(theme: ThemeConfig): Promise<string | null> {
  const path = await save({ title: '导出主题', defaultPath: filenames.theme, filters });
  return path ? invoke('theme_export', { path, theme }) : null;
}

export async function importTheme(): Promise<ThemeConfig | null> {
  const path = await open({ title: '选择知迹主题文件', multiple: false, directory: false, filters });
  return path ? invoke('theme_import', { path }) : null;
}
