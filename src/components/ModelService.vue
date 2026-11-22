<script setup lang="ts">
import { computed, reactive, watch } from 'vue';
import { Braces, Check, EyeOff, Layers3, MessagesSquare, PlugZap, RefreshCw } from 'lucide-vue-next';
import { apiModels, apiTest } from '../api';
import { createServiceDraft, modelPresets, normalizedEndpoint, providers, serviceInput } from '../model-services';
import type { ServiceDraft, ServiceKind } from '../model-services';
import type { ConnectionTest, ServiceConfig, ServiceProvider } from '../types';
import AppSelect from './AppSelect.vue';
import openaiMark from '../assets/providers/openai.svg';
import deepseekMark from '../assets/providers/deepseek.svg';

const props = defineProps<{ kind: ServiceKind; saved: ServiceConfig; disabled: boolean }>();
const draft = defineModel<ServiceDraft>({ required: true });
const tests = reactive({ running: false, result: null as ConnectionTest | null, error: '' });
const models = reactive({ running: false, ids: [] as string[], loaded: false, error: '', manual: false });
let testVersion = 0;
let modelsVersion = 0;
let cached: Partial<Record<ServiceProvider, ServiceDraft>> = {};
const providerOptions = computed(() => providers.filter(item => props.kind === 'chat' || item.value !== 'deepseek'));
const savedKey = computed(() => props.saved.hasApiKey && normalizedEndpoint(draft.value.baseUrl) === normalizedEndpoint(props.saved.baseUrl));
const modelOptions = computed(() => {
  const preset = modelPresets[props.kind][draft.value.provider];
  const options = draft.value.provider === 'custom' ? models.ids.map(value => ({ value, label: value })) : [...preset];
  if (draft.value.model && !options.some(option => option.value === draft.value.model)) {
    options.unshift({ value: draft.value.model, label: draft.value.model });
  }
  if (!draft.value.model) options.unshift({ value: '', label: '选择模型或手动输入' });
  if (draft.value.provider === 'custom') options.push({ value: '__manual__', label: '手动输入模型 ID…' });
  return options;
});

watch(() => props.saved, () => { cached = {}; });
watch(() => [draft.value.baseUrl, draft.value.model, draft.value.apiKey, draft.value.clearKey], () => {
  testVersion++;
  tests.result = null;
  tests.error = '';
}, { flush: 'sync' });
watch(() => [draft.value.provider, draft.value.baseUrl, draft.value.apiKey, draft.value.clearKey], () => {
  modelsVersion++;
  models.ids = [];
  models.loaded = false;
  models.error = '';
  models.manual = false;
}, { flush: 'sync' });

function chooseProvider(provider: string) {
  const selected = provider as ServiceProvider;
  cached[draft.value.provider] = { ...draft.value };
  draft.value = cached[selected] ?? createServiceDraft(props.kind, undefined, selected);
}

function chooseModel(value: string) {
  models.manual = value === '__manual__';
  if (!models.manual) draft.value.model = value;
}

async function fetchModels() {
  if (models.running) return;
  const version = ++modelsVersion;
  models.running = true;
  models.error = '';
  try {
    const input = serviceInput(draft.value, props.saved);
    const ids = await apiModels(props.kind, input.baseUrl, input.apiKey);
    if (version !== modelsVersion) return;
    models.ids = ids;
    models.loaded = true;
  } catch (error) {
    if (version === modelsVersion) models.error = String(error);
  } finally { models.running = false; }
}

async function testService() {
  if (tests.running) return;
  const version = ++testVersion;
  tests.running = true;
  tests.error = '';
  tests.result = null;
  try {
    const result = await apiTest(props.kind, serviceInput(draft.value, props.saved));
    if (version === testVersion) tests.result = result;
  } catch (error) {
    if (version === testVersion) tests.error = String(error);
  } finally { tests.running = false; }
}
</script>

<template>
  <section class="model-service" :class="{ 'vector-service': kind === 'embedding' }" :data-testid="`${kind}-service`">
    <header class="model-service-heading">
      <MessagesSquare v-if="kind === 'chat'" :size="18" /><Layers3 v-else :size="18" />
      <h2>{{ kind === 'chat' ? '聊天模型' : '向量模型' }}</h2>
    </header>
    <fieldset :disabled="disabled" :class="{ 'custom-fields': draft.provider === 'custom' }">
      <legend class="sr-only">{{ kind === 'chat' ? '聊天模型配置' : '向量模型配置' }}</legend>
      <div class="provider-field">
        <span :id="`${kind}-provider-label`" class="model-field-label">服务提供商</span>
        <div v-if="kind === 'chat'" class="provider-choices" role="group" :aria-labelledby="`${kind}-provider-label`">
          <button v-for="provider in providerOptions" :key="provider.value" type="button" class="provider-choice" :class="{ selected: draft.provider === provider.value }" :aria-pressed="draft.provider === provider.value" :title="provider.description" :data-testid="`${kind}-provider-${provider.value}`" @click="chooseProvider(provider.value)">
            <span class="provider-choice-title"><Braces v-if="provider.value === 'custom'" :size="18" /><img v-else :src="provider.value === 'openai' ? openaiMark : deepseekMark" :class="{ 'openai-provider-mark': provider.value === 'openai' }" alt="" width="18" height="18" /><strong>{{ provider.label }}</strong><i aria-hidden="true" /></span>
          </button>
        </div>
        <AppSelect v-else :model-value="draft.provider" :options="providerOptions" :disabled="disabled" aria-label="向量服务提供商" data-testid="embedding-provider" @update:model-value="chooseProvider" />
      </div>
      <label v-if="draft.provider === 'custom'" class="field base-field">服务地址
        <input v-model="draft.baseUrl" type="url" required placeholder="https://llm.example.com/v1" spellcheck="false" :data-testid="`${kind}-base-url`" />
      </label>
      <template v-for="field in draft.provider === 'custom' ? ['key', 'model'] : ['model', 'key']" :key="field">
        <div v-if="field === 'model'" class="field model-field">
          <div class="model-label-row"><label :id="`${kind}-model-label`">模型</label><button v-if="draft.provider === 'custom'" type="button" class="text-button" :disabled="models.running || !draft.baseUrl.trim() || disabled" :data-testid="`${kind}-fetch-models`" @click="fetchModels"><RefreshCw :size="13" :class="{ spinning: models.running }" />{{ models.running ? '正在获取…' : '获取模型列表' }}</button></div>
          <AppSelect :model-value="models.manual ? '__manual__' : draft.model" :options="modelOptions" :disabled="disabled" :aria-labelledby="`${kind}-model-label`" :aria-label="kind === 'chat' ? '聊天模型' : '向量模型'" :data-testid="`${kind}-model`" @update:model-value="chooseModel" />
          <label v-if="models.manual" class="manual-model">模型 ID<input v-model="draft.model" required placeholder="输入服务支持的模型 ID" spellcheck="false" :data-testid="`${kind}-manual-model`" /></label>
          <p v-if="models.loaded && !models.ids.length" class="model-hint" role="status">服务返回的模型列表为空，可手动输入模型 ID。</p>
          <p v-if="models.error" class="inline-error" role="alert" :data-testid="`${kind}-models-error`">{{ models.error }}。可手动输入模型 ID。</p>
        </div>
        <div v-else class="service-key">
          <label class="field">API 密钥
            <span class="password-field"><input v-model="draft.apiKey" type="password" autocomplete="new-password" spellcheck="false" :disabled="draft.clearKey" :placeholder="savedKey && !draft.clearKey ? '••••••••••••••••' : '输入 API 密钥'" :data-testid="`${kind}-api-key`" /><EyeOff :size="18" aria-hidden="true" /></span>
          </label>
          <small class="model-hint">密钥加密保存在此设备。</small>
          <label v-if="savedKey" class="clear-key"><input v-model="draft.clearKey" type="checkbox" :data-testid="`${kind}-clear-key`" />移除已存密钥</label>
        </div>
      </template>
      <footer class="model-service-footer"><button type="button" class="button" :disabled="tests.running || disabled || !draft.model.trim()" :data-testid="`test-${kind}`" @click="testService"><RefreshCw v-if="tests.running" :size="15" class="spinning" /><PlugZap v-else :size="16" />{{ tests.running ? '正在测试…' : '测试连接' }}</button></footer>
    </fieldset>
    <p v-if="tests.result" class="connection-result" role="status" :data-testid="`${kind}-test-result`"><Check :size="14" />{{ tests.result.message }} · {{ tests.result.latencyMs }} ms</p>
    <p v-if="tests.result" class="connection-usage" :data-testid="`${kind}-test-usage`">{{ tests.result.usageReported ? `本次请求输入 ${tests.result.inputTokens}，输出 ${tests.result.outputTokens} tokens` : '本次请求未返回用量' }}</p>
    <p v-if="tests.error" class="inline-error" role="alert" :data-testid="`${kind}-test-error`">{{ tests.error }}</p>
  </section>
</template>

<style scoped>
.model-service { min-width: 0; }
.model-service-heading { display: flex; align-items: center; gap: 10px; }
.model-service-heading > svg { flex-shrink: 0; color: var(--accent-text); }
.model-service-heading h2 { font-size: var(--font-size-section); font-weight: 600; }
.model-service fieldset { min-width: 0; padding: 0; margin: 20px 0 0; border: 0; }
.sr-only { position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
.model-field-label, .model-service .field { color: var(--fg); font-size: var(--font-size-label); font-weight: 550; }
.model-field-label { display: block; margin-bottom: 8px; }
.provider-choices { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
.provider-choice { min-width: 0; min-height: 40px; padding: 9px 10px; text-align: left; border: 1px solid var(--line); border-radius: 7px; background: var(--bg); }
.provider-choice:hover { border-color: var(--accent-line); background: var(--soft); }
.provider-choice.selected { border-color: var(--accent); background: var(--soft); }
.provider-choice-title { display: flex; align-items: center; gap: 8px; }
.provider-choice-title strong { font-size: var(--font-size-body); font-weight: 550; }
.provider-choice-title > img, .provider-choice-title > svg { flex-shrink: 0; }
:global(html[style*="color-scheme: dark"] .openai-provider-mark) { filter: invert(1); }
.provider-choice-title > svg { color: var(--muted); }
.provider-choice-title i { width: 13px; height: 13px; border: 1px solid var(--line); border-radius: 50%; margin-left: auto; flex-shrink: 0; }
.selected .provider-choice-title i { border: 4px solid var(--accent); background: var(--bg); }
.model-service .field { margin-top: 18px; gap: 8px; }
.model-label-row { display: flex; align-items: center; justify-content: space-between; gap: 10px; min-height: 18px; }
.model-label-row .text-button { padding: 0; font-size: var(--font-size-label); color: var(--accent-text); }
.model-service :deep(.app-select) { width: 100%; min-height: 36px; height: 36px; padding: 7px 10px; font-size: var(--font-size-body); font-weight: 400; border-radius: 7px; }
.model-service .field input { height: 36px; border-radius: 7px; padding: 7px 10px; background: var(--bg); color: var(--fg); font-size: var(--font-size-body); font-weight: 400; }
.service-key { min-width: 0; }
.model-service input[type=url], .manual-model input { font-family: Menlo, Consolas, monospace; font-size: 12px; }
.password-field { display: flex; align-items: center; position: relative; }
.password-field input { padding-right: 38px !important; }
.password-field svg { position: absolute; right: 11px; color: var(--muted); pointer-events: none; }
.model-service .clear-key { margin-top: 9px; }
.manual-model { display: grid; gap: 8px; font-size: var(--font-size-label); font-weight: 400; color: var(--muted); margin-top: 3px; }
.model-hint { font-size: 11px; line-height: 1.7; font-weight: 400; color: var(--muted); margin-top: 7px; }
.model-service-footer { display: flex; align-items: center; flex-wrap: wrap; gap: 12px; border-top: 1px solid var(--line); margin-top: 22px; padding-top: 16px; }
.vector-service { border: 1px solid var(--line); border-radius: 10px; padding: 21px; background: var(--side); }
@media (min-width: 1051px) {
  .model-service:not(.vector-service) .custom-fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); column-gap: 14px; align-items: start; }
  .custom-fields .provider-field, .custom-fields .model-field, .custom-fields .model-service-footer { grid-column: 1 / -1; }
}
@media (max-width: 1250px) and (min-width: 1051px) {
  .provider-choice { padding-left: 8px; padding-right: 8px; }
  .provider-choice-title { gap: 6px; }
  .provider-choice-title i { display: none; }
}
@media (max-width: 700px) {
  .provider-choices { gap: 6px; }
  .provider-choice { padding: 9px 7px; }
  .provider-choice-title { gap: 6px; }
  .provider-choice-title i { display: none; }
  .vector-service { padding: 18px; }
}
</style>
