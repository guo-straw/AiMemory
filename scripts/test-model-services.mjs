import assert from 'node:assert/strict';
import { chatModels, createServiceDraft, serviceInput } from '../src/model-services.ts';

const saved = { baseUrl: 'https://api.openai.com/v1', model: 'gpt-4.1-mini', hasApiKey: true };
const existing = createServiceDraft('chat', saved);
assert.equal(existing.provider, 'openai');
assert.equal(existing.model, saved.model);
assert.equal(serviceInput(existing, saved).apiKey, undefined);
existing.model = 'gpt-6.2-sol';
assert.equal(serviceInput(existing, saved).apiKey, undefined);
existing.clearKey = true;
assert.equal(serviceInput(existing, saved).apiKey, '');

const deepseek = createServiceDraft('chat', undefined, 'deepseek');
assert.equal(deepseek.model, 'deepseek-flash');
assert.equal(serviceInput(deepseek, saved).apiKey, '');
deepseek.apiKey = 'new-provider-key';
assert.equal(serviceInput(deepseek, saved).apiKey, 'new-provider-key');
assert.equal(createServiceDraft('chat', { ...saved, baseUrl: 'https://api.deepseek.com/v1' }).provider, 'deepseek');
assert.equal(createServiceDraft('chat', { ...saved, provider: 'custom' }).provider, 'custom');

const custom = createServiceDraft('embedding', { ...saved, baseUrl: 'http://localhost:1234/v1', model: 'local-embed' });
assert.equal(custom.provider, 'custom');
assert.equal(custom.model, 'local-embed');
assert.equal(createServiceDraft('chat').model, 'gpt-6.2-astra');
assert.equal(createServiceDraft('embedding').model, 'text-embedding-3-large');
assert.deepEqual(chatModels.openai.map(model => model.value), ['gpt-6.2-astra', 'gpt-6.2-sol', 'gpt-6.1-sol', 'gpt-6.1-terra']);
console.log('模型预设、旧配置保留、跨服务密钥隔离检查通过');
