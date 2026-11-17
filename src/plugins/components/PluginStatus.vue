<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, useId, watch } from 'vue';
import ResponsivePopup from '../../components/ResponsivePopup.vue';
import PluginImage from './PluginImage.vue';
import type { PluginManifest, UiNode } from '../types';
const props = defineProps<{ node: UiNode; manifest: PluginManifest }>();
const open = ref(false);
const root = ref<HTMLElement>();
const trigger = ref<HTMLButtonElement>();
const id = useId();
watch(open, async value => {
  if (!value) return;
  await nextTick();
  const menus = document.querySelectorAll<HTMLElement>(`[data-plugin-status-menu="${id}"]`);
  for (const menu of menus) if (menu.getBoundingClientRect().width) { menu.querySelector<HTMLButtonElement>('button')?.focus(); break; }
});
async function close() {
  if (!open.value) return;
  open.value = false;
  await nextTick();
  trigger.value?.focus();
}
function outside(event: PointerEvent) {
  if (open.value && !root.value?.contains(event.target as Node) && !(event.target as Element).closest?.(`[data-plugin-status-menu="${id}"]`)) void close();
}
function keydown(event: KeyboardEvent) {
  if (open.value && event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); void close(); }
}
watch(() => props.node.children?.length, length => { if (!length) void close(); });
onMounted(() => { document.addEventListener('pointerdown', outside); window.addEventListener('keydown', keydown, true); });
onUnmounted(() => { document.removeEventListener('pointerdown', outside); window.removeEventListener('keydown', keydown, true); });
</script>
<template>
  <div ref="root" class="plugin-status" :class="`plugin-status-${node.tone}`">
    <ResponsivePopup v-if="node.children?.length" :open="open" panel-class="plugin-status-popup" @close="close">
      <template #trigger><button ref="trigger" class="plugin-status-trigger" :aria-label="node.label" :title="node.label" aria-haspopup="dialog" :aria-expanded="open" @click.stop="open ? close() : open = true"><PluginImage :manifest="manifest" monochrome tint="currentColor" /></button></template>
      <section :data-plugin-status-menu="id" role="dialog" :aria-label="node.title || manifest.name" class="plugin-status-content">
        <header><h3>{{ node.title || manifest.name }}</h3></header>
        <slot />
        <footer><button class="control-button" aria-label="Close plugin menu" @click="close">Close</button></footer>
      </section>
    </ResponsivePopup>
    <span v-else role="status" :aria-label="node.label" :title="node.label" class="plugin-status-static"><PluginImage :manifest="manifest" monochrome tint="currentColor" /></span>
  </div>
</template>
<style scoped>
.plugin-status { display: flex; align-items: center; justify-content: center; align-self: center; color: var(--color-accent-text); --plugin-icon-size: 19px; }
.plugin-status-trigger,.plugin-status-static { box-sizing: border-box; display: flex; align-items: center; justify-content: center; width: 32px; height: 32px; padding: 0; border: 0; border-radius: 0; background: transparent; color: inherit; }
.plugin-status-trigger { cursor: pointer; }
.plugin-status-trigger:focus-visible { outline: 2px solid var(--color-accent); outline-offset: 2px; }
.plugin-status-active .plugin-status-trigger :deep(.plugin-image),.plugin-status-active .plugin-status-static :deep(.plugin-image) { animation: plugin-status-pulse 1.4s ease-in-out infinite; }
.plugin-status-error { color: var(--color-text-muted); }
.plugin-status-success { color: var(--color-accent-text); }
.plugin-status-content { box-sizing: border-box; min-width: 0; max-width: 100%; padding: var(--space-4); color: var(--color-text); overflow-wrap: anywhere; }
header { margin-bottom: var(--space-4); }
h3 { margin: 0; }
footer { display: flex; justify-content: flex-end; margin-top: var(--space-4); }
.plugin-status-content :deep(.plugin-group),.plugin-status-content :deep(.plugin-results) { min-width: 0; }
.plugin-status-content :deep(.plugin-result) { display: grid; grid-template-columns: auto minmax(0, 1fr); align-items: start; min-width: 0; }
.plugin-status-content :deep(.plugin-result-actions) { grid-column: 1 / -1; width: auto; min-width: 0; justify-content: flex-start; }
.plugin-status-content :deep(.control-button) { max-width: 100%; white-space: normal; overflow-wrap: anywhere; }
@keyframes plugin-status-pulse { 50% { opacity: .35; } }
@media(max-width: 600px) { .plugin-status-trigger,.plugin-status-static { border: 1px solid var(--color-surface-hover); border-radius: 20px; background: var(--color-surface); } }
@media(prefers-reduced-motion: reduce) { .plugin-status-active .plugin-status-trigger :deep(.plugin-image),.plugin-status-active .plugin-status-static :deep(.plugin-image) { animation: none; } }
</style>
<style>
.responsive-popup-desktop.plugin-status-popup { box-sizing: border-box; width: min(380px, calc(100vw - 24px)); min-width: 0; max-width: calc(100vw - 24px); }
</style>
