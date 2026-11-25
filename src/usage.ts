import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import type { ClientSource } from './types';

export interface UsageTokens { input: number; cached: number | null; cacheWrite: number; output: number; reasoning: number | null }
export interface UsageSummary {
  records: number; sessions: number; input: number; cached: number; cacheWrite: number; output: number; reasoning: number;
  cacheKnownInput: number; reasoningKnownOutput: number; pricedTokens: number; pricedRecords: number; cost: number;
}
export interface UsageGroup { key: string; summary: UsageSummary }
export interface UsageBucket { start: number; end: number; label: string }
export interface UsageFilters { client?: ClientSource | null; start: number; end: number; project: string | null; model: string | null; role: string | null; buckets: UsageBucket[] }
export interface UsageStatus { scanning: boolean; completed: number; total: number; lastSyncedAt: number | null; errors: string[] }
export interface UsagePreferences { autoScan: boolean; intervalSeconds: number }
export interface UsageQuotaWindow { used_percent: number; window_minutes: number | null; resets_at: number | null }
export interface UsageQuota {
  at: number; threadId: string;
  data: { limit_id?: string; limit_name?: string; primary: UsageQuotaWindow | null; secondary: UsageQuotaWindow | null; plan_type?: string };
}
export interface UsageOverview {
  summary: UsageSummary; models: UsageGroup[]; projects: UsageGroup[]; roles: string[]; availableModels: string[]; availableProjects: string[];
  trend: Array<{ label: string; start: number; summary: UsageSummary }>;
  quota: UsageQuota | null; issueCount: number; status: UsageStatus; preferences: UsagePreferences; directory: string; directories: Record<ClientSource, string>;
}
export interface UsageSession {
  client: ClientSource; id: string; title: string; parentId: string | null; role: string; model: string; project: string; firstAt: number; lastAt: number; summary: UsageSummary;
}
export interface UsageRecord {
  client: ClientSource; id: string; threadId: string; turnId: string; at: number; model: string; provider: string; tier: string; project: string;
  role: string; tokens: UsageTokens; cost: number | null; file: string; line: number; basis: string;
}
export interface UsageCostGroup { price: UsagePrice; input: number; cached: number; cacheWrite: number; output: number }
export interface UsageDetail { session: UsageSession; records: UsageRecord[]; totalRecords: number; costParts: number[]; costGroups: UsageCostGroup[]; memories: Array<{ id: string; title: string }> }
export interface UsagePrice {
  model: string; provider: string; tier: string; input: number | null; cached: number | null; cacheWrite: number | null; output: number | null; source: string; custom: boolean;
}
export interface UsageIssue { client: ClientSource; file: string; line: number; reason: string }

export const usageOverview = (filters: UsageFilters) => invoke<UsageOverview>('usage_overview', { filters });
export const usageScan = (full = false) => invoke<UsageStatus>('usage_scan', { full });
export const usageSessions = (filters: UsageFilters, page: number, sort: string) => invoke<{ items: UsageSession[]; total: number }>('usage_sessions', { filters, page, sort });
export const usageDetail = (filters: UsageFilters, threadId: string, page = 0) => invoke<UsageDetail>('usage_detail', { filters, threadId, page });
export const usagePricesGet = () => invoke<UsagePrice[]>('usage_prices_get');
export const usagePricesSave = (prices: UsagePrice[]) => invoke<void>('usage_prices_save', { prices });
export const usagePreferencesSave = (preferences: UsagePreferences) => invoke<UsagePreferences>('usage_preferences_save', { preferences });
export const usageIssues = (client?: ClientSource | null) => invoke<UsageIssue[]>('usage_issues', { client: client ?? null });
export const usageOpenSource = (recordId: string) => invoke<void>('usage_open_source', { recordId });
export async function usageExport(filters: UsageFilters) {
  const title = filters.client ? `导出 ${clientLabel(filters.client)} 用量` : '导出全部来源用量';
  const path = await save({ title, defaultPath: filters.client ? `${filters.client}-usage.csv` : 'usage.csv', filters: [{ name: 'CSV', extensions: ['csv'] }] });
  if (path) await invoke('usage_export', { filters, path });
  return !!path;
}

export const clientLabel = (client: ClientSource = 'codex') => client === 'harness' ? 'DeepSeek Harness' : 'Codex';
export const totalTokens = (s: UsageSummary) => s.input + s.output;
export const cacheRate = (s: UsageSummary) => s.cacheKnownInput ? `${(s.cached / s.cacheKnownInput * 100).toFixed(1)}%` : '—';
export const number = (n: number) => n.toLocaleString('zh-CN');
export function compact(n: number) {
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(2)}M`;
  if (n >= 1e4) return `${(n / 1e3).toFixed(1)}K`;
  return number(n);
}
export const money = (cost: number | null, precision = 2) => cost === null ? '未定价' : `$${cost.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: Math.max(precision, cost && cost < .01 ? 5 : 2) })}`;
export const referenceCost = (s: UsageSummary) => !s.records ? '—' : s.pricedRecords ? money(s.cost) : '未定价';
export const roleLabel = (role: string) => ({ main: '主对话', agent: '子代理', system: '系统辅助' }[role] ?? role);
export const projectLabel = (path: string) => path === '*' ? '多个项目' : path.split(/[\\/]/).filter(Boolean).at(-1) || '未识别项目';
export const modelLabel = (model: string) => model === '*' ? '多个模型' : model || '未识别模型';
export const dateTime = (at: number) => new Date(at).toLocaleString('zh-CN', { hour12: false, month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
export const time = (at: number) => new Date(at).toLocaleTimeString('zh-CN', { hour12: false, hour: '2-digit', minute: '2-digit' });
export function dateInput(date: Date) { return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`; }
export function timeBuckets(start: number, end: number, hourly = false): UsageBucket[] {
  const result: UsageBucket[] = [];
  const cursor = new Date(start);
  while (cursor.getTime() < end) {
    const at = cursor.getTime();
    const label = hourly ? `${String(cursor.getHours()).padStart(2, '0')}:00` : `${cursor.getMonth() + 1}/${cursor.getDate()}`;
    if (hourly) cursor.setTime(at + 3_600_000);
    else cursor.setDate(cursor.getDate() + 1);
    result.push({ start: at, end: Math.min(end, cursor.getTime()), label });
  }
  return result;
}
