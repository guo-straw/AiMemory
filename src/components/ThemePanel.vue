<script setup lang="ts">
import { computed, nextTick, reactive, ref, watch } from 'vue';
import { ArrowLeft, BookOpen, Copy, Download, RefreshCw, Upload } from 'lucide-vue-next';
import { DEFAULT_THEME, THEME_PRESETS } from '../theme';
import type { ThemeConfig } from '../theme';
import { exportTheme, importTheme } from '../transfer';
import AppSelect from './AppSelect.vue';

const props = defineProps<{ theme: ThemeConfig; saving?: boolean; embedded?: boolean }>();
const emit = defineEmits<{ change: [theme: ThemeConfig]; back: [] }>();
const draft = reactive<ThemeConfig>({ ...props.theme });
const colorNames = { accent: '强调色', background: '背景', foreground: '前景' };
const accents = [
  { value: '#3566F0', name: '蓝色' }, { value: '#7564BE', name: '紫色' },
  { value: '#137968', name: '青绿' }, { value: '#9B5421', name: '赭色' },
];
const invalid = reactive({ accent: false, background: false, foreground: false });
const message = ref('');
const fileError = ref('');
const busy = ref(false);
const copiedJson = ref('');
const copyField = ref<HTMLTextAreaElement>();
const customPreset = computed(() => !THEME_PRESETS.some(theme => theme.preset === draft.preset));
const presetOptions = computed(() => [
  ...THEME_PRESETS.map(theme => ({ value: theme.preset, label: theme.preset })),
  ...(customPreset.value ? [{ value: draft.preset, label: draft.preset }] : []),
]);
const accentOptions = [...accents.map(accent => ({ value: accent.value, label: accent.name })), { value: 'custom', label: '自定义' }];
const fontOptions = [{ value: 'system', label: '系统' }, { value: 'serif', label: '衬线' }, { value: 'mono', label: '等宽' }];
const accentChoice = computed(() => accents.some(accent => accent.value === draft.accent) ? draft.accent : 'custom');

watch(() => props.theme, theme => {
  Object.assign(draft, theme);
  for (const key of ['accent', 'background', 'foreground'] as const) invalid[key] = false;
}, { deep: true });

function apply(theme: ThemeConfig) {
  Object.assign(draft, theme);
  copiedJson.value = '';
  fileError.value = '';
  for (const key of ['accent', 'background', 'foreground'] as const) invalid[key] = false;
  emit('change', { ...theme });
}

function change(key: 'accent' | 'background' | 'foreground' | 'font', value: string) {
  const next = { ...props.theme, [key]: value, preset: '自定义' } as ThemeConfig;
  const preset = THEME_PRESETS.find(theme => ['accent', 'background', 'foreground', 'font'].every(field => theme[field as keyof ThemeConfig] === next[field as keyof ThemeConfig]));
  apply(preset ? { ...preset } : next);
  message.value = '外观已应用到整个工作区';
}

function colorInput(key: 'accent' | 'background' | 'foreground', value: string, commit = false) {
  draft[key] = value;
  const full = (value.startsWith('#') ? value : '#' + value).toUpperCase();
  if (/^#[0-9A-F]{6}$/.test(full)) {
    invalid[key] = false;
    if (props.theme[key] !== full) change(key, full);
    else draft[key] = full;
  } else if (commit) invalid[key] = true;
}

function choosePreset(value: string) {
  const preset = THEME_PRESETS.find(theme => theme.preset === value);
  if (preset) apply({ ...preset });
  message.value = `已应用「${draft.preset}」`;
}

function chooseAccent(color: string) {
  if (color !== 'custom') change('accent', color);
}

async function copyTheme() {
  const json = JSON.stringify({ type: 'aimemory.theme', version: 1, theme: props.theme }, null, 2) + '\n';
  try {
    await navigator.clipboard.writeText(json);
    copiedJson.value = '';
    message.value = '已复制主题配置 JSON';
  } catch {
    copiedJson.value = json;
    await nextTick();
    copyField.value?.focus();
    copyField.value?.select();
    message.value = '请复制已选中的主题 JSON';
  }
}

async function exportFile() {
  busy.value = true;
  fileError.value = '';
  try {
    const path = await exportTheme(props.theme);
    if (path) message.value = `已导出主题 · ${path}`;
  } catch (error) {
    fileError.value = String(error);
  } finally { busy.value = false; }
}

async function importFile() {
  busy.value = true;
  fileError.value = '';
  try {
    const theme = await importTheme();
    if (theme) {
      apply(theme);
      message.value = '已导入主题';
    }
  } catch (error) {
    fileError.value = String(error);
  } finally { busy.value = false; }
}
</script>

<template>
  <section class="theme-page" data-testid="theme-panel">
    <button v-if="!embedded" type="button" class="text-button theme-back" @click="emit('back')"><ArrowLeft :size="17" />返回设置</button>
    <header v-if="!embedded" class="theme-page-heading"><h1>主题与外观</h1></header>
    <div class="theme-layout">
      <div class="theme-form">
        <header class="theme-toolbar">
          <label for="theme-preset-control">主题预设</label>
          <div class="theme-tools">
            <button type="button" class="text-button" aria-label="复制主题配置" @click="copyTheme"><Copy :size="15" />复制</button>
            <button type="button" class="text-button" :disabled="busy" aria-label="导出主题" @click="exportFile"><Download :size="15" />导出</button>
          </div>
        </header>
        <div class="theme-preset-row">
          <div class="theme-preset"><span class="theme-aa" aria-hidden="true">Aa</span><AppSelect id="theme-preset-control" :model-value="draft.preset" :options="presetOptions" aria-label="主题预设" data-testid="theme-preset" @update:model-value="choosePreset" /></div>
          <span class="theme-saving" role="status">{{ saving ? '正在保存…' : '更改自动保存' }}</span>
        </div>
        <div class="theme-rows">
          <div v-for="(label, key) in colorNames" :key="key" class="theme-row">
            <div><h2>{{ label }}</h2></div>
            <div class="theme-controls">
              <AppSelect v-if="key === 'accent'" :model-value="accentChoice" :options="accentOptions" class="theme-pill" aria-label="强调色选项" data-testid="theme-accent-choice" @update:model-value="chooseAccent" />
              <div class="theme-color-pill">
                <input type="color" :value="theme[key]" :aria-label="`${label}色盘`" @input="change(key, ($event.target as HTMLInputElement).value.toUpperCase())" />
                <input :value="draft[key]" class="theme-hex" :aria-label="`${label} HEX`" :aria-invalid="invalid[key]" :data-testid="`theme-${key}`" maxlength="7" spellcheck="false" autocomplete="off" @input="colorInput(key, ($event.target as HTMLInputElement).value)" @change="colorInput(key, ($event.target as HTMLInputElement).value, true)" @keydown.enter.prevent="colorInput(key, ($event.target as HTMLInputElement).value, true)" @keydown.escape="draft[key] = theme[key]; invalid[key] = false" />
              </div>
            </div>
            <p v-if="invalid[key]" class="theme-field-error" role="alert">请输入六位 HEX 颜色，例如 #3566F0。</p>
          </div>
          <div class="theme-row"><div><h2>字体</h2></div><div class="theme-controls"><AppSelect :model-value="draft.font" :options="fontOptions" class="theme-pill" aria-label="界面字体" data-testid="theme-font" @update:model-value="value => change('font', value)" /></div></div>
        </div>
        <footer class="theme-actions"><button type="button" class="button" :disabled="busy" data-testid="theme-import" @click="importFile"><Upload :size="15" />导入主题</button><button type="button" class="text-button" data-testid="theme-reset" @click="apply({ ...DEFAULT_THEME }); message = '已恢复默认 ChatGPT 主题'"><RefreshCw :size="15" />恢复默认</button></footer>
        <p v-if="fileError" class="inline-error" role="alert">{{ fileError }}</p>
        <p class="theme-message" role="status" aria-live="polite">{{ message }}</p>
        <textarea v-if="copiedJson" ref="copyField" class="theme-json" :value="copiedJson" readonly aria-label="可手动复制的主题 JSON" />
      </div>
      <aside class="theme-preview" aria-label="外观实时预览">
        <h2>实时预览</h2>
        <section class="theme-reading-preview" aria-label="阅读预览">
          <header class="preview-toolbar"><BookOpen :size="16" /><span>记忆工作台</span></header>
          <div class="preview-content">
            <span class="preview-kind">技术决策</span>
            <h3>任务进度改用短轮询</h3>
            <p>异步导出每 2 秒查询一次任务状态，完成后停止查询。</p>
            <blockquote>页面卸载时清理计时器，避免重复请求。</blockquote>
            <a href="#" @click.prevent>查看关联记忆 →</a>
          </div>
        </section>
      </aside>
    </div>
  </section>
</template>

<style scoped>
.theme-page { flex: 1; min-width: 0; min-height: 0; overflow-y: auto; padding: 34px 44px 40px; color: var(--fg); background: var(--bg); }
.theme-back { padding: 0; color: var(--accent-text); font-size: var(--font-size-body); margin-bottom: 24px; }
.theme-page-heading { margin-bottom: 24px; }
.theme-page-heading h1 { font-size: var(--font-size-page); font-weight: 650; }
.theme-layout { display: grid; grid-template-columns: minmax(420px, 1.2fr) minmax(280px, 1fr); align-items: start; gap: 36px; max-width: 1100px; }
.theme-form { min-width: 0; }
.theme-toolbar { display: flex; align-items: center; justify-content: space-between; gap: 16px; margin-bottom: 10px; }
.theme-toolbar > label { font-size: var(--font-size-label); font-weight: 550; }
.theme-tools { display: flex; gap: 8px; }
.theme-tools .text-button { font-size: var(--font-size-label); padding: 5px; }
.theme-preset-row { display: flex; align-items: center; gap: 16px; margin-bottom: 20px; }
.theme-preset { display: flex; align-items: center; gap: 10px; border: 1px solid var(--line); border-radius: 8px; min-height: 40px; flex: 1; padding: 5px 12px 5px 8px; }
.theme-aa { display: grid; place-items: center; width: 27px; height: 27px; color: var(--accent-text); background: var(--soft); border-radius: 5px; font: 600 14px -apple-system, BlinkMacSystemFont, sans-serif; }
.theme-preset :deep(.app-select) { flex: 1; min-width: 0; border: 0; padding: 0; min-height: 28px; background: transparent; color: var(--fg); font-size: var(--font-size-body); }
.theme-saving { flex-shrink: 0; color: var(--faint); font-size: var(--font-size-label); }
.theme-rows { border-top: 1px solid var(--line); }
.theme-row { display: grid; grid-template-columns: minmax(120px, 1fr) auto; column-gap: 16px; align-items: center; min-height: 82px; padding: 16px 0; border-bottom: 1px solid var(--line); }
.theme-row h2 { font-size: var(--font-size-body); line-height: 1.5; font-weight: 550; }
.theme-controls { display: flex; align-items: center; justify-content: flex-end; gap: 8px; }
.theme-controls :deep(.theme-pill), .theme-color-pill { border: 1px solid var(--line); border-radius: 7px; background: var(--bg); color: var(--fg); min-height: 36px; padding: 6px 10px; font-size: var(--font-size-body); }
.theme-controls :deep(.theme-pill) { min-width: 89px; }
.theme-color-pill { display: flex; gap: 9px; align-items: center; }
.theme-color-pill input[type=color] { flex-shrink: 0; appearance: none; width: 19px; height: 19px; padding: 0; border: 1px solid var(--line); border-radius: 4px; overflow: hidden; background: none; cursor: pointer; }
.theme-color-pill input[type=color]::-webkit-color-swatch-wrapper { padding: 0; }
.theme-color-pill input[type=color]::-webkit-color-swatch { border: 0; border-radius: 3px; }
.theme-hex { width: 77px; padding: 0; border: 0; font: 12px Menlo, Consolas, monospace; text-transform: uppercase; background: transparent; outline-offset: 4px; }
.theme-hex[aria-invalid=true] { text-decoration: underline wavy var(--danger); }
.theme-row .theme-field-error { grid-column: 1 / -1; margin-top: 8px; color: var(--danger); font-size: var(--font-size-label); }
.theme-actions { display: flex; align-items: center; gap: 14px; margin-top: 18px; }
.theme-actions .text-button { font-size: var(--font-size-label); }
.theme-message { color: var(--muted); font-size: var(--font-size-label); line-height: 1.7; margin-top: 12px; overflow-wrap: anywhere; }
.theme-message:empty { margin-top: 0; }
.theme-json { display: block; width: 100%; height: 160px; background: var(--surface); color: var(--fg); border: 1px solid var(--line); border-radius: 7px; padding: 13px; margin-top: 15px; font: 12px/1.6 Menlo, Consolas, monospace; resize: vertical; }
.theme-preview { min-width: 0; padding: 21px; border: 1px solid var(--line); border-radius: 10px; background: var(--side); }
.theme-preview > h2 { font-size: var(--font-size-body); font-weight: 550; margin-bottom: 18px; }
.theme-reading-preview { overflow: hidden; border: 1px solid var(--line); border-radius: 8px; background: var(--bg); }
.preview-toolbar { display: flex; align-items: center; gap: 9px; padding: 14px 18px; border-bottom: 1px solid var(--line); color: var(--muted); font-size: var(--font-size-label); }
.preview-toolbar > svg { color: var(--accent-text); }
.preview-content { padding: 23px 20px; }
.preview-kind { display: inline-block; padding: 3px 7px; border-radius: 4px; color: var(--accent-text); background: var(--soft); font-size: 11px; }
.preview-content h3 { margin-top: 13px; font-size: var(--font-size-section); line-height: 1.6; font-weight: 600; }
.preview-content p { font-size: var(--font-size-body); margin-top: 13px; line-height: 1.8; color: var(--fg); }
.preview-content blockquote { margin: 16px 0; padding: 10px 12px; border-left: 3px solid var(--accent); background: var(--surface); color: var(--muted); font-size: var(--font-size-label); line-height: 1.8; }
.preview-content a { display: inline-block; color: var(--accent-text); font-size: var(--font-size-label); text-decoration: none; }
.preview-content a:hover { text-decoration: underline; }
@media (max-width: 1050px) { .theme-page { padding: 30px; } .theme-layout { grid-template-columns: minmax(0, 1fr); max-width: 680px; gap: 24px; } }
@media (max-width: 700px) { .theme-page { padding: 27px 24px; } .theme-row { grid-template-columns: minmax(0, 1fr); gap: 12px; } .theme-controls { justify-content: flex-start; flex-wrap: wrap; } .theme-actions { flex-wrap: wrap; } .theme-preset-row { flex-wrap: wrap; gap: 10px; } .theme-preset { flex-basis: 100%; } .theme-preview { padding: 18px; } }
</style>
