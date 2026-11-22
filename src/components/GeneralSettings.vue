<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { Power } from 'lucide-vue-next';
import { autostartGet, autostartSet } from '../api';

defineProps<{ disabled: boolean }>();
const enabled = ref(false);
const loading = ref(true);
const saving = ref(false);
const error = ref('');
const loaded = ref(false);

onMounted(async () => {
  try {
    enabled.value = await autostartGet();
    loaded.value = true;
  } catch (cause) {
    error.value = String(cause);
  } finally {
    loading.value = false;
  }
});

async function toggle(event: Event) {
  const input = event.target as HTMLInputElement;
  const next = input.checked;
  input.checked = enabled.value;
  saving.value = true;
  error.value = '';
  try {
    await autostartSet(next);
    enabled.value = next;
  } catch (cause) {
    error.value = String(cause);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <section class="settings-content-grid" data-testid="general-settings">
    <div class="settings-main-panel">
      <header class="settings-section-title"><Power :size="18" /><h2>启动设置</h2></header>
      <div class="settings-option-row">
        <div><h3>开机自启</h3><p class="startup-description">登录电脑后自动启动知迹，更改立即生效。</p></div>
        <label class="toggle-label">
          <input :checked="enabled" type="checkbox" role="switch" aria-label="开机自启" :disabled="disabled || loading || saving || !loaded" data-testid="autostart-enabled" @change="toggle" />
          <span role="status">{{ loading ? '正在读取…' : saving ? '正在设置…' : !loaded ? '读取失败' : enabled ? '已开启' : '已关闭' }}</span>
        </label>
      </div>
      <p v-if="error" class="inline-error" role="alert">{{ error }}</p>
    </div>
  </section>
</template>

<style scoped>
.startup-description { margin-top: 7px; color: var(--muted); font-size: var(--font-size-label); line-height: 1.8; }
</style>
