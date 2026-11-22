import type { ServiceConfig, ServiceInput, ServiceProvider } from './types';

export type ServiceKind = 'chat' | 'embedding';
export interface ServiceDraft {
  provider: ServiceProvider;
  baseUrl: string;
  model: string;
  apiKey: string;
  clearKey: boolean;
}

export const providers = [
  { value: 'openai' as const, label: 'OpenAI', description: '官方模型服务', baseUrl: 'https://api.openai.com/v1' },
  { value: 'deepseek' as const, label: 'DeepSeek', description: '官方模型服务', baseUrl: 'https://api.deepseek.com' },
  { value: 'custom' as const, label: '自定义', description: 'OpenAI 兼容服务', baseUrl: '' },
];

export const chatModels = {
  openai: [
    { value: 'gpt-6.2-astra', label: 'GPT-6.2 Astra' },
    { value: 'gpt-6.2-sol', label: 'GPT-6.2 Sol' },
    { value: 'gpt-6.1-sol', label: 'GPT-6.1 Sol' },
    { value: 'gpt-6.1-terra', label: 'GPT-6.1 Terra' },
  ],
  deepseek: [
    { value: 'deepseek-flash', label: 'DeepSeek V4.1 Flash' },
    { value: 'deepseek-v4-pro', label: 'DeepSeek V4 Pro' },
  ],
  custom: [],
};
export const embeddingModels = [
  { value: 'text-embedding-3-large', label: 'text-embedding-3-large' },
  { value: 'text-embedding-3-small', label: 'text-embedding-3-small' },
];

export const modelPresets = {
  chat: chatModels,
  embedding: { openai: embeddingModels, deepseek: [], custom: [] },
};

export function normalizedEndpoint(value: string): string {
  return value.trim().replace(/\/+$/, '');
}

export function createServiceDraft(kind: ServiceKind, saved?: ServiceConfig, provider?: ServiceProvider): ServiceDraft {
  const baseUrl = normalizedEndpoint(saved?.baseUrl ?? '');
  const inferred = providers.find(item => item.value !== 'custom' &&
    (baseUrl === item.baseUrl || item.value === 'deepseek' && baseUrl === `${item.baseUrl}/v1`))?.value ?? 'custom';
  const selected = provider ?? saved?.provider ?? (saved ? inferred : 'openai');
  return {
    provider: selected,
    baseUrl: saved?.baseUrl ?? providers.find(item => item.value === selected)!.baseUrl,
    model: saved?.model ?? modelPresets[kind][selected][0]?.value ?? '',
    apiKey: '',
    clearKey: false,
  };
}

export function serviceInput(draft: ServiceDraft, saved: ServiceConfig): ServiceInput {
  const sameEndpoint = normalizedEndpoint(draft.baseUrl) === normalizedEndpoint(saved.baseUrl);
  const key = draft.clearKey ? '' : draft.apiKey.trim();
  return {
    provider: draft.provider,
    baseUrl: draft.baseUrl.trim(),
    model: draft.model.trim(),
    ...(draft.clearKey || key || (!sameEndpoint && saved.hasApiKey) ? { apiKey: key } : {}),
  };
}
