<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { X } from 'lucide-vue-next';
defineProps<{ title: string; wide?: boolean }>();
const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLDialogElement>();
onMounted(() => dialog.value?.showModal());
</script>

<template>
  <Teleport to="body">
    <dialog ref="dialog" class="usage-drawer" :class="{ wide }" aria-modal="true" :aria-label="title" @cancel.prevent="emit('close')" @click="($event.target === dialog) && emit('close')">
      <header><h2>{{ title }}</h2><button class="icon-button" aria-label="关闭面板" @click="emit('close')"><X :size="19" /></button></header>
      <div class="usage-drawer-body"><slot /></div>
      <footer v-if="$slots.footer"><slot name="footer" /></footer>
    </dialog>
  </Teleport>
</template>

<style scoped>
.usage-drawer { position: fixed; inset: 0 0 0 auto; width: min(580px, 100vw); max-width: 100vw; height: 100dvh; max-height: 100dvh; margin: 0; padding: 0; border: 0; border-left: 1px solid var(--line); color: var(--fg); background: var(--bg); box-shadow: -10px 0 40px #00000013; overflow: hidden; }
.usage-drawer[open] { display: flex; flex-direction: column; }
.usage-drawer.wide { width: min(850px, 100vw); }
.usage-drawer::backdrop { background: #161b2a55; }
header { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 20px 24px; border-bottom: 1px solid var(--line); flex: 0 0 auto; }
h2 { margin: 0; font-size: 14px; font-weight: 600; }
.usage-drawer-body { flex: 1; min-height: 0; overflow: auto; padding: 24px; overscroll-behavior: contain; }
footer { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: 12px; padding: 16px 24px; border-top: 1px solid var(--line); flex: 0 0 auto; }
@media (max-width: 600px) { .usage-drawer-body { padding: 20px; } header, footer { padding: 16px 20px; } }
</style>
