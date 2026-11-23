import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import type { MemoryGraph, MemoryInput, MemoryRecord, MemorySummary, Repository, RepositoryInput, Workspace } from './types';
import type { AppSettings, SettingsInput, ServiceInput, ConnectionTest, ArchiveStatus } from './types';
import type { SearchRequest, SearchResponse, McpInfo, McpHttpStatus, AppUpdateResult, UpdateInstaller } from './types';

export function workspaceOpen(path?: string): Promise<Workspace> {
  return invoke('workspace_open', { path: path ?? null });
}

export function workspaceMigrate(path: string): Promise<Workspace> {
  return invoke('workspace_migrate', { path });
}

export function workspaceRefresh(): Promise<Workspace> {
  return invoke('workspace_refresh');
}

export function repositoryAdd(input: RepositoryInput): Promise<Repository> {
  return invoke('repository_add', { input });
}

export function repositoryRename(id: string, name: string): Promise<Workspace> {
  return invoke('repository_rename', { id, name });
}

export function repositoryDelete(id: string): Promise<Workspace> {
  return invoke('repository_delete', { id });
}

export function memorySearch(repositoryId: string | null, query: string): Promise<MemorySummary[]> {
  return invoke('memory_search', { repositoryId, query });
}

export function memoryRead(id: string): Promise<MemoryRecord> {
  return invoke('memory_read', { id });
}

export function memoryGraph(): Promise<MemoryGraph> {
  return invoke('memory_graph');
}

export function memorySave(input: MemoryInput): Promise<MemoryRecord> {
  return invoke('memory_save', { input });
}

export function memoryAdoptSuggestion(id: string, sourceId: string, revision: string): Promise<MemoryRecord> {
  return invoke('memory_adopt_suggestion', { id, sourceId, revision });
}

export async function chooseWorkspace(migrate = false): Promise<string | null> {
  return open({ directory: true, multiple: false, title: migrate ? '选择迁移目标（空目录）' : '打开已有 Markdown 记忆库' });
}

export function openMemoryFile(id: string): Promise<void> {
  return invoke('memory_open_file', { id });
}

export function settingsGet(): Promise<AppSettings> {
  return invoke('settings_get');
}

export function settingsSave(input: SettingsInput): Promise<AppSettings> {
  return invoke('settings_save', { input });
}

export function autostartGet(): Promise<boolean> {
  return invoke('plugin:autostart|is_enabled');
}

export function autostartSet(enabled: boolean): Promise<void> {
  return invoke(enabled ? 'plugin:autostart|enable' : 'plugin:autostart|disable');
}

export function apiTest(service: 'chat' | 'embedding', input: ServiceInput): Promise<ConnectionTest> {
  return invoke('api_test', { service, input });
}

export function apiModels(service: 'chat' | 'embedding', baseUrl: string, apiKey?: string): Promise<string[]> {
  return invoke('api_models', { service, baseUrl, apiKey: apiKey ?? null });
}

export function archiveStatus(): Promise<ArchiveStatus> {
  return invoke('archive_status');
}

export function archiveRetry(): Promise<void> {
  return invoke('archive_retry');
}

export async function chooseCodexDirectory(): Promise<string | null> {
  return open({ directory: true, multiple: false, title: '选择 Codex 数据目录（包含 sessions）' });
}

export function semanticSearch(request: SearchRequest): Promise<SearchResponse> {
  return invoke('semantic_search', { request });
}

export function mcpInfo(): Promise<McpInfo> {
  return invoke('mcp_info');
}

export function mcpHttpStatus(): Promise<McpHttpStatus> {
  return invoke('mcp_http_status');
}

export function mcpHttpStart(): Promise<McpHttpStatus> {
  return invoke('mcp_http_start');
}

export function mcpHttpStop(): Promise<McpHttpStatus> {
  return invoke('mcp_http_stop');
}

export function checkAppUpdate(token: string | null): Promise<AppUpdateResult> {
  return invoke('app_update_check', { token });
}

export async function downloadAppUpdate(installer: UpdateInstaller, token: string | null): Promise<string | null> {
  const path = await save({ title: '保存知迹 KnowTrace 安装包', defaultPath: installer.name });
  if (!path) return null;
  await invoke('app_update_download', { assetId: installer.assetId, expectedSize: installer.size, path, token });
  return path;
}
