<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, useAttrs, useId, watch } from 'vue';
import { Check, ChevronDown } from 'lucide-vue-next';

defineOptions({ inheritAttrs: false });
const props = withDefaults(defineProps<{
  modelValue: string;
  options: Array<{ value: string; label: string }>;
  disabled?: boolean;
}>(), { disabled: false });
const emit = defineEmits<{ 'update:modelValue': [value: string] }>();
const attrs = useAttrs();
const trigger = ref<HTMLButtonElement>();
const list = ref<HTMLElement>();
const opened = ref(false);
const activeIndex = ref(0);
const scrollTop = ref(0);
const rowHeight = 36;
const menuHeight = ref(0);
const menuStyle = ref<Record<string, string>>({});
const uid = useId();
const listId = `${uid}-options`;
const selectedLabel = computed(() => props.options.find(option => option.value === props.modelValue)?.label ?? props.modelValue);
const firstVisible = computed(() => Math.max(0, Math.floor(scrollTop.value / rowHeight) - 3));
const visibleOptions = computed(() => props.options.slice(firstVisible.value, firstVisible.value + Math.ceil(menuHeight.value / rowHeight) + 6));
const listTestId = computed(() => attrs['data-testid'] ? `${attrs['data-testid']}-options` : undefined);

function positionMenu() {
  const rect = trigger.value!.getBoundingClientRect();
  const style = getComputedStyle(trigger.value!);
  const theme = Object.fromEntries(['--bg', '--fg', '--line', '--muted', '--soft', '--accent', '--accent-text', '--accent-line', '--font'].map(name => [name, style.getPropertyValue(name)]));
  const margin = 8;
  const gap = 6;
  const desiredHeight = Math.min(props.options.length * rowHeight + 10, 288);
  const below = Math.max(0, window.innerHeight - rect.bottom - gap - margin);
  const above = Math.max(0, rect.top - gap - margin);
  const upwards = below < desiredHeight && above > below;
  const height = Math.min(desiredHeight, upwards ? above : below);
  const width = Math.min(Math.max(rect.width, 160), window.innerWidth - margin * 2);
  const left = Math.min(Math.max(margin, rect.left), window.innerWidth - width - margin);
  menuHeight.value = height;
  menuStyle.value = {
    ...theme, colorScheme: style.colorScheme,
    left: `${left}px`, top: `${upwards ? rect.top - gap - height : rect.bottom + gap}px`,
    width: `${width}px`, height: `${height}px`,
  };
}

function revealActive() {
  if (!list.value) return;
  const top = activeIndex.value * rowHeight;
  const visibleHeight = list.value.clientHeight - 8;
  if (top < list.value.scrollTop) list.value.scrollTop = top;
  else if (top + rowHeight > list.value.scrollTop + visibleHeight) list.value.scrollTop = top + rowHeight - visibleHeight;
  scrollTop.value = list.value.scrollTop;
}

async function openMenu() {
  if (props.disabled || !props.options.length) return;
  activeIndex.value = Math.max(0, props.options.findIndex(option => option.value === props.modelValue));
  scrollTop.value = 0;
  positionMenu();
  opened.value = true;
  await nextTick();
  revealActive();
}

function closeMenu(restoreFocus = false) {
  opened.value = false;
  if (restoreFocus) trigger.value?.focus({ preventScroll: true });
}

function selectOption(index: number) {
  emit('update:modelValue', props.options[index].value);
  closeMenu(true);
}

function keydown(event: KeyboardEvent) {
  if (event.isComposing) return;
  if (event.key === 'Tab') { closeMenu(); return; }
  if (event.key === 'Escape') {
    if (opened.value) { event.preventDefault(); event.stopPropagation(); closeMenu(true); }
    return;
  }
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End', 'Enter', ' '].includes(event.key)) return;
  event.preventDefault();
  event.stopPropagation();
  if (!opened.value) { void openMenu(); return; }
  if (event.key === 'Enter' || event.key === ' ') { selectOption(activeIndex.value); return; }
  if (event.key === 'Home') activeIndex.value = 0;
  else if (event.key === 'End') activeIndex.value = props.options.length - 1;
  else activeIndex.value = (activeIndex.value + (event.key === 'ArrowDown' ? 1 : props.options.length - 1)) % props.options.length;
  void nextTick(revealActive);
}

function pointerOutside(event: PointerEvent) {
  if (!opened.value) return;
  const path = event.composedPath();
  if (!path.includes(trigger.value!) && !path.includes(list.value!)) closeMenu();
}

function pageScroll(event: Event) {
  if (event.target !== list.value) closeMenu();
}

function resize() { closeMenu(); }

watch(() => props.disabled, disabled => { if (disabled) closeMenu(); });
watch(() => props.options, () => {
  if (!opened.value) return;
  if (!props.options.length) { closeMenu(); return; }
  activeIndex.value = Math.min(activeIndex.value, props.options.length - 1);
  positionMenu();
  void nextTick(revealActive);
});
onMounted(() => {
  document.addEventListener('pointerdown', pointerOutside);
  window.addEventListener('scroll', pageScroll, true);
  window.addEventListener('resize', resize);
});
onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', pointerOutside);
  window.removeEventListener('scroll', pageScroll, true);
  window.removeEventListener('resize', resize);
});
</script>

<template>
  <button ref="trigger" v-bind="attrs" type="button" class="app-select" role="combobox" aria-haspopup="listbox" :aria-expanded="opened" :aria-controls="listId" :aria-activedescendant="opened ? `${listId}-${activeIndex}` : undefined" :disabled="disabled || !options.length" :title="selectedLabel" @click="opened ? closeMenu() : openMenu()" @keydown="keydown">
    <span class="app-select-label">{{ selectedLabel }}</span><ChevronDown :size="14" aria-hidden="true" />
  </button>
  <Teleport to="body">
    <div v-if="opened" :id="listId" ref="list" class="app-select-list" role="listbox" :aria-label="attrs['aria-label'] as string | undefined" :style="menuStyle" :data-testid="listTestId" @scroll="scrollTop = ($event.target as HTMLElement).scrollTop" @keydown="keydown">
      <div class="app-select-spacer" role="presentation" :style="{ height: `${options.length * rowHeight}px` }">
        <button v-for="(option, offset) in visibleOptions" :id="`${listId}-${firstVisible + offset}`" :key="option.value" type="button" role="option" tabindex="-1" class="app-select-option" :class="{ active: firstVisible + offset === activeIndex }" :aria-selected="option.value === modelValue" :aria-posinset="firstVisible + offset + 1" :aria-setsize="options.length" :title="option.label" :style="{ top: `${(firstVisible + offset) * rowHeight}px` }" @pointermove="activeIndex = firstVisible + offset" @click="selectOption(firstVisible + offset)">
          <span>{{ option.label }}</span><Check v-if="option.value === modelValue" :size="14" aria-hidden="true" />
        </button>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.app-select { display: inline-flex; align-items: center; justify-content: space-between; gap: 9px; min-width: 0; max-width: 100%; min-height: 35px; padding: 7px 10px; border: 1px solid var(--line); border-radius: 7px; background: var(--bg, #fff); color: var(--fg, #1a1c1f); text-align: left; font-size: 13px; line-height: 1.5; }
.app-select:hover:not(:disabled) { border-color: var(--accent-line, #b8caff); }
.app-select[aria-expanded=true] { border-color: var(--accent); }
.app-select > svg { flex-shrink: 0; color: var(--muted); }
.app-select-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.app-select-list { position: fixed; z-index: 10000; box-sizing: border-box; overflow-y: auto; overflow-x: hidden; padding: 4px; border: 1px solid var(--line); border-radius: 8px; background: var(--bg, #fff); color: var(--fg, #1a1c1f); box-shadow: 0 8px 28px #00000020; overscroll-behavior: contain; font: 13px/1.5 var(--font, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif); }
.app-select-spacer { position: relative; }
.app-select-option { position: absolute; left: 0; right: 0; display: flex; align-items: center; justify-content: space-between; gap: 12px; box-sizing: border-box; width: 100%; height: 36px; min-height: 36px; margin: 0; padding: 0 10px; border: 0; border-radius: 5px; background: transparent; color: inherit; font: inherit; text-align: left; }
.app-select-option > span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.app-select-option > svg { flex-shrink: 0; color: var(--accent-text); }
.app-select-option.active { background: var(--soft); color: var(--accent-text); }
</style>
