<script setup lang="ts">
import {
  ChevronRight, CircleAlert, Clock3, Ellipsis, FileText, Layers,
  Layers2, Pause, Power, Radio, ScanLine,
} from 'lucide-vue-next';
import type { ArchiveSidebarState } from '../archive-sidebar';

defineProps<{ state: ArchiveSidebarState }>();
const emit = defineEmits<{ navigate: [target: 'archive' | 'settings'] }>();
const icons = {
  radio: Radio, 'scan-line': ScanLine, 'clock-3': Clock3, 'file-text': FileText,
  layers: Layers, 'layers-2': Layers2, 'circle-alert': CircleAlert,
  ellipsis: Ellipsis, power: Power, pause: Pause,
};
const destinations = { archive: '查看归档与用量', settings: '打开设置与接入' };
</script>

<template>
  <div class="archive-status">
    <button
      type="button"
      class="status-action"
      :class="state.tone"
      :aria-disabled="state.target === null"
      :tabindex="state.target === null ? -1 : 0"
      :title="state.target ? state.label + ' · ' + destinations[state.target] : state.label"
      @click="state.target && emit('navigate', state.target)"
    >
      <span class="status-icon" aria-hidden="true">
        <component :is="icons[state.icon]" :size="18" />
      </span>
      <span class="status-text">
        <span class="status-caption">Codex 自动归档</span>
        <strong class="status-label" aria-live="polite" aria-atomic="true">{{ state.label }}</strong>
      </span>
      <ChevronRight class="status-chevron" :size="13" aria-hidden="true" />
    </button>
  </div>
</template>

<style scoped>
.archive-status {
  position: relative;
  flex: 0 0 80px;
  margin-top: auto;
  padding: 16px 0 8px;
}
.archive-status::before {
  content: '';
  position: absolute;
  inset: 8px 7px auto;
  border-top: 1px solid var(--line);
}
.status-action {
  display: grid;
  grid-template-columns: 32px minmax(0, 1fr) 13px;
  align-items: center;
  width: 100%;
  height: 56px;
  padding: 0 6px 0 8px;
  text-align: left;
  background: transparent;
  border: 0;
  border-radius: 8px;
}
.status-action:not([aria-disabled="true"]):hover { background: var(--hover); }
.status-action[aria-disabled="true"] { cursor: default; }
.status-action[aria-disabled="true"] .status-chevron { visibility: hidden; }
.status-icon {
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: 9px;
  color: var(--muted);
  background: var(--hover, var(--line));
}
.ready .status-icon {
  color: var(--success);
  background: color-mix(in srgb, var(--success) 12%, var(--side));
}
.working .status-icon { color: var(--accent-text); background: var(--soft); }
.failed .status-icon { color: var(--danger); background: var(--danger-soft); }
.failed .status-label { color: var(--danger); }
.status-text {
  display: grid;
  gap: 2px;
  min-width: 0;
  padding: 0 3px 0 11px;
}
.status-caption {
  color: var(--muted);
  font-size: 11px;
  font-weight: 500;
  line-height: 16px;
}
.status-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 14px;
  font-weight: 600;
  line-height: 20px;
}
.status-chevron { color: var(--faint); }
</style>
