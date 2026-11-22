<script setup lang="ts">
import { computed, ref } from 'vue';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { openUrl } from '@tauri-apps/plugin-opener';

const props = defineProps<{ markdown: string; title?: string }>();
const linkError = ref('');
const html = computed(() => {
  const tokens = marked.lexer(props.markdown);
  const firstIndex = tokens.findIndex(token => token.type !== 'space');
  const first = tokens[firstIndex];
  if (first?.type === 'heading' && first.depth === 1 && first.text.trim() === props.title?.trim()) {
    tokens.splice(firstIndex, 1);
  }
  const conclusion = tokens.findIndex(token => token.type === 'heading' && token.depth === 2 && token.text === '结论');
  if (conclusion < 0) return DOMPurify.sanitize(marked.parser(tokens));
  const nextSection = tokens.findIndex((token, index) => index > conclusion && token.type === 'heading' && token.depth <= 2);
  const end = nextSection < 0 ? tokens.length : nextSection;
  const content = marked.parser(tokens.slice(0, conclusion)) +
    '<section class="memory-conclusion"><h2>结论</h2>' +
    marked.parser(tokens.slice(conclusion + 1, end)) + '</section>' +
    marked.parser(tokens.slice(end));
  return DOMPurify.sanitize(content);
});

async function followLink(event: MouseEvent) {
  const anchor = (event.target as Element).closest('a');
  if (!anchor) return;
  event.preventDefault();
  const href = anchor.getAttribute('href') || '';
  if (!/^https?:\/\//i.test(href) && !/^mailto:/i.test(href)) return;
  try {
    await openUrl(href);
    linkError.value = '';
  } catch (error) {
    linkError.value = String(error);
  }
}
</script>

<template>
  <div class="markdown" @click="followLink" v-html="html" />
  <p v-if="linkError" class="inline-error" role="alert">{{ linkError }}</p>
</template>
