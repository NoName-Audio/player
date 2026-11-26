<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue';
import ResponsivePopup from '../../components/ResponsivePopup.vue';
import AppIcon from '../../components/AppIcon.vue';
import type { PluginManifest, UiNode } from '../types';
import CoverImage from '../../components/CoverImage.vue';
import { coverUrl } from '../../utils/covers';
import SettingsToggle from '../../components/SettingsToggle.vue';
import PluginStatus from './PluginStatus.vue';
defineProps<{ node: UiNode; manifest?: PluginManifest; fields: Record<string, string | boolean>; busy: boolean; pendingButton?: string; path?: string; selection?: ReadonlySet<string>; unavailable?: ReadonlySet<string> }>();
const emit = defineEmits<{ action: [action: string, payload?: Record<string, unknown>, button?: string]; select: [id: string, checked: boolean] }>();
const fileError = ref('');
const readingFile = ref(false);
const filePicker = ref<HTMLInputElement>();
async function importTextFile(event: Event, action: string) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = '';
  if (!file || readingFile.value) return;
  fileError.value = '';
  if (!file.size || file.size > 64 * 1024) {
    fileError.value = 'Choose a nonempty text file no larger than 64 KiB.';
    return;
  }
  readingFile.value = true;
  try {
    emit('action', action, { text: await file.text() });
  } catch {
    fileError.value = 'Could not read the selected file.';
  } finally {
    readingFile.value = false;
  }
}
const resultButton = (path: string | undefined, resourceId: string, action: string) => JSON.stringify([path || '', resourceId, action]);
const openDownload = ref<string>();
const formats = [
  { title: 'Audio', label: 'WebM/Opus', video: false },
  { title: 'Video', label: 'MP4', video: true },
];
const previewChoices = new Map<string, { video: boolean; saveData: boolean }>();
function chooseDownload(action: string, resourceId: string, button: string, video: boolean, saveData: boolean) {
  const choice = { video, saveData };
  if (action === 'play') previewChoices.set(resourceId, choice);
  closeDownload();
  emit('action', action, { resourceId, ...choice }, button);
}
function resultAction(action: { id: string; downloadOptions?: boolean }, resourceId: string, button: string) {
  const choice = action.downloadOptions && action.id === 'save'
    ? previewChoices.get(resourceId) ?? { video: false, saveData: false }
    : undefined;
  closeDownload();
  emit('action', action.id, { resourceId, ...choice }, button);
}
function closeDownload() { openDownload.value = undefined; }
onMounted(() => window.addEventListener('click', closeDownload));
onUnmounted(() => window.removeEventListener('click', closeDownload));
</script>
<template>
  <div v-if="node.type === 'group'" class="plugin-group" :class="{ 'plugin-card-grid': node.layout === 'cards', 'plugin-settings-card': node.layout === 'card' }">
    <h3 v-if="node.title">{{ node.title }}</h3>
    <PluginTree v-for="(child, index) in node.children || []" :key="index" :node="child" :manifest="manifest" :fields="fields" :busy="busy" :pending-button="pendingButton" :path="`${path || ''}/${index}`" @action="(action, payload, button) => emit('action', action, payload, button)" />
  </div>
  <div v-else-if="node.type === 'account'" class="plugin-account">
    <span class="plugin-account-avatar" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.6"><circle cx="12" cy="8" r="3"/><path d="M5 20v-2a7 7 0 0 1 14 0v2"/></svg>
      <img v-if="node.avatarUrl" :key="node.avatarUrl" :src="node.avatarUrl" alt="" referrerpolicy="no-referrer" @error="($event.target as HTMLImageElement).hidden = true" />
    </span>
    <span>{{ node.text }}</span>
  </div>
  <p v-else-if="node.type === 'text' && node.text" :class="{ 'plugin-error': node.tone === 'error' }">{{ node.text }}</p>
  <PluginStatus v-else-if="node.type === 'status' && manifest" :node="node" :manifest="manifest"><PluginTree v-for="(child, index) in node.children || []" :key="index" :node="child" :manifest="manifest" :fields="fields" :busy="busy" :pending-button="pendingButton" :path="`${path || ''}/${index}`" @action="(action, payload, button) => emit('action', action, payload, button)" /></PluginStatus>
  <div v-else-if="node.type === 'input' && node.inputType === 'text-file' && node.action" class="plugin-field form-field">
    <button type="button" class="control-button" :disabled="busy || readingFile" :aria-busy="busy || readingFile" @click="filePicker?.click()">{{ node.label }}</button>
    <input ref="filePicker" hidden type="file" accept=".txt,text/plain" :disabled="busy || readingFile" @change="importTextFile($event, node.action)" />
    <span v-if="fileError" role="alert" class="plugin-error">{{ fileError }}</span>
  </div>
  <label v-else-if="node.type === 'input' && node.key" class="plugin-field form-field">
    <span>{{ node.label }}</span><input v-model="fields[node.key]" class="form-input" :type="node.inputType === 'password' ? 'password' : 'text'" :autocomplete="node.inputType === 'password' ? 'new-password' : 'off'" :disabled="busy" />
  </label>
  <button v-else-if="node.type === 'button' && node.action" class="control-button" :disabled="busy || node.disabled" :aria-busy="busy && pendingButton === `${path || ''}/button`" @click="emit('action', node.action!, node.payload, `${path || ''}/button`)">{{ node.label }}</button>
  <div v-else-if="node.type === 'toggle' && node.action" class="plugin-field plugin-toggle"><span>{{ node.label }}</span><SettingsToggle :model-value="Boolean(node.value)" :label="node.label || node.action" :disabled="busy || node.disabled" :aria-busy="busy && pendingButton === `${path || ''}/toggle`" @update:model-value="value => emit('action', node.action!, { ...node.payload, value }, `${path || ''}/toggle`)" /></div>
  <div v-else-if="node.type === 'results'" class="plugin-results">
    <div v-for="item in node.items || []" :key="item.id" class="plugin-result">
      <input v-if="selection" class="form-checkbox" type="checkbox" :aria-label="`Select ${item.title}`" :checked="selection.has(item.id)" :disabled="busy || unavailable?.has(item.id)" @change="emit('select', item.id, ($event.target as HTMLInputElement).checked)" />
      <CoverImage class="track-cover-sm" :src="coverUrl(item.coverHash, 'small')" :loading="item.coverLoading" />
      <div class="plugin-result-copy">
        <strong>{{ item.title }}</strong><span v-if="item.subtitle">{{ item.subtitle }}</span><span v-if="item.status">{{ item.status }}</span>
        <div v-if="item.progress !== undefined" class="plugin-result-progress">
          <progress :aria-label="`Downloading ${item.title}`" :value="item.progress ?? undefined" max="100" />
          <span>{{ item.progress === null ? 'Downloading…' : `${Math.floor(item.progress)}%` }}</span>
        </div>
      </div>
      <div class="plugin-result-actions">
        <template v-for="action in item.actions || []" :key="action.id">
          <ResponsivePopup v-if="action.downloadOptions && action.id !== 'save'" anchored :open="openDownload === resultButton(path, item.id, action.id)" @close="closeDownload">
            <template #trigger>
              <button class="control-button" :disabled="busy || action.disabled" :aria-expanded="openDownload === resultButton(path, item.id, action.id)" @click.stop="openDownload = openDownload === resultButton(path, item.id, action.id) ? undefined : resultButton(path, item.id, action.id)">
                {{ action.label }} <AppIcon name="dropdown" :size="12" />
              </button>
            </template>
            <div v-for="format in formats" :key="format.title" class="download-menu-group" role="group" :aria-label="format.title">
              <div class="download-menu-heading">{{ format.title }}</div>
              <button class="download-menu-item" :disabled="busy || action.disabled" @click="chooseDownload(action.id, item.id, resultButton(path, item.id, action.id), format.video, false)">{{ format.label }}</button>
              <button class="download-menu-item" :disabled="busy || action.disabled" @click="chooseDownload(action.id, item.id, resultButton(path, item.id, action.id), format.video, true)">Save data</button>
            </div>
          </ResponsivePopup>
          <button v-else class="control-button" :disabled="busy || action.disabled" :aria-busy="busy && pendingButton === resultButton(path, item.id, action.id)" @click="resultAction(action, item.id, resultButton(path, item.id, action.id))">{{ action.label }}</button>
        </template>
      </div>
    </div>
  </div>
</template>
<style scoped>
.download-menu-group + .download-menu-group { border-top: 1px solid var(--color-border); }
.download-menu-heading { padding: var(--space-2) var(--space-4) 0; color: var(--color-text-muted); font-size: 12px; }
.download-menu-item { display: block; width: 100%; min-height: 40px; text-align: left; background: none; border: none; color: var(--color-text); font-size: var(--fs-button); padding: var(--space-2) var(--space-4); cursor: pointer; }
.download-menu-item:hover, .download-menu-item:focus-visible { background: var(--color-surface-hover); }
.plugin-group { display: grid; gap: var(--space-4); }.plugin-group h3 { margin: 0; }.plugin-group p { color: var(--color-text-muted); margin: 0; line-height: 1.5; }
.plugin-field { min-width: 0; }
.plugin-toggle { display: flex; align-items: center; justify-content: space-between; }.plugin-error { color: var(--color-danger-text) !important; }
.plugin-group > .control-button { justify-self: start; }.plugin-results { display: grid; gap: var(--space-2); }.plugin-result { display: flex; gap: var(--space-3); align-items: center; padding: var(--space-3) 0; border-bottom: 1px solid var(--color-border); }
.plugin-result > .track-cover-sm { width: 36px; height: 36px; border-radius: 3px; flex-shrink: 0; }
.plugin-result-copy { min-width: 0; flex: 1; display: grid; gap: var(--space-1); }.plugin-result-copy strong,.plugin-result-copy span { overflow-wrap: anywhere; }.plugin-result-copy span { color: var(--color-text-muted); font-size: 12px; }.plugin-result-actions { display: flex; flex-wrap: wrap; gap: var(--space-2); }
.plugin-result-progress { display: flex; align-items: center; gap: var(--space-2); }
.plugin-result-progress progress { flex: 1; min-width: 0; height: 6px; accent-color: var(--color-accent); }
.plugin-result-progress span { flex-shrink: 0; font-variant-numeric: tabular-nums; }
@media(max-width: 650px) { .plugin-result { flex-wrap: wrap; }.plugin-result > .track-cover-sm { width: 32px; height: 32px; }.plugin-result-actions { width: 100%; justify-content: flex-end; } }
</style>
<style scoped>
.plugin-card-grid { grid-template-columns: repeat(auto-fit, minmax(min(100%, 17rem), 1fr)); align-items: start; }
.plugin-settings-card { min-width: 0; align-content: start; overflow-wrap: anywhere; }
.plugin-settings-card :deep(.control-button) { width: 100%; min-height: 44px; white-space: normal; }
.plugin-account { display: flex; align-items: center; gap: 12px; }
.plugin-account-avatar { position: relative; display: grid; place-items: center; width: 44px; height: 44px; flex: 0 0 44px; border: 1px solid currentColor; border-radius: 50%; overflow: hidden; }
.plugin-account-avatar img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
</style>
