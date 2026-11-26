<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { usePluginHost, validateUiNode } from '../host';
import type { SearchRefresh, SearchResult, SearchItem, UiContribution } from '../types';
import PluginTree from './PluginTree.vue';
import AppIcon from '../../components/AppIcon.vue';
import ResponsivePopup from '../../components/ResponsivePopup.vue';
const props = defineProps<{ contribution: UiContribution; query: string; sharedRequest?: object }>();
const emit = defineEmits<{ sharedSearchStarted: [] }>();
const host = usePluginHost(); const items = ref<SearchItem[]>([]); const error = ref(''); const loading = ref(false); const submitted = ref('');
const loadingDirection = ref<'new' | 'next' | 'previous'>('new');
const providerPending = ref(false);
const searchId = ref<string>();
let refreshSequence = 0;
const actionBusy = ref(false); const pendingButton = ref<string>();
let sequence = 0; let disposed = false;
const currentCursor = ref<string | null>(null); const nextCursor = ref<string | null>(null); const previousCursors = ref<Array<string | null>>([]);
const batch = ref<SearchResult['batch']>();
const playlist = computed(() => Boolean(batch.value));
function libraryStatus(ids: string[]) {
  const plugin = host.plugins.value.find(p => p.manifest.id === props.contribution.pluginId && p.enabled);
  if (!plugin) throw new Error('Plugin is not enabled');
  return invoke<string[]>('plugin_call', { pluginId: plugin.manifest.id, generation: plugin.generation, method: 'library.status', args: { ids } });
}
const selected = ref(new Set<string>());
const batchMenuOpen = ref(false);
function closeBatchMenu() { batchMenuOpen.value = false; }
onMounted(() => window.addEventListener('click', closeBatchMenu));
onUnmounted(() => window.removeEventListener('click', closeBatchMenu));
const present = ref(new Set<string>());
const checking = ref(false);
const statusError = ref('');
let statusRequest = 0;
const rows = computed(() => playlist.value ? items.value.map(item => present.value.has(item.id) ? {
  ...item, status: 'Already in library', actions: item.actions?.filter(action => action.id !== batch.value?.action),
} : item) : items.value);
const available = computed(() => rows.value.filter(item => !present.value.has(item.id) && item.actions?.some(action => action.id === batch.value?.action && action.downloadOptions && !action.disabled)));
const batchDisabled = computed(() => loading.value || providerPending.value || checking.value || Boolean(statusError.value) || actionBusy.value || !available.value.some(item => selected.value.has(item.id)));
watch(batchDisabled, disabled => { if (disabled) closeBatchMenu(); });
const unavailable = computed(() => new Set(rows.value.filter(item => !available.value.includes(item)).map(item => item.id)));
const ids = computed(() => JSON.stringify(batch.value ? items.value.map(item => item.id) : []));
async function checkLibrary() {
  const request = ++statusRequest;
  const requested: string[] = JSON.parse(ids.value);
  checking.value = requested.length > 0;
  statusError.value = '';
  if (!requested.length) { present.value.clear(); return; }
  try {
    const found = await libraryStatus(requested);
    if (request !== statusRequest || disposed) return;
    present.value = new Set(found);
    for (const id of found) selected.value.delete(id);
  } catch (e) {
    if (request === statusRequest && !disposed) statusError.value = String(e);
  } finally {
    if (request === statusRequest) checking.value = false;
  }
}
watch(ids, () => { selected.value.clear(); void checkLibrary(); });
const unlisten = listen('library-changed', () => { if (batch.value) void checkLibrary(); });
onUnmounted(() => { void unlisten.then(stop => stop(), () => {}); });
function select(id: string, checked: boolean) {
  if (checked) selected.value.add(id); else selected.value.delete(id);
}
async function downloadSelected(video: boolean) {
  closeBatchMenu();
  const operation = batch.value;
  if (!operation || batchDisabled.value) return;
  const seq = sequence;
  const candidates = available.value.filter(item => selected.value.has(item.id));
  actionBusy.value = true;
  error.value = '';
  const failed: string[] = [];
  try {
    for (const item of candidates) {
      if (seq !== sequence || disposed) break;
      try {
        const found = await libraryStatus([item.id]);
        if (seq !== sequence || disposed) break;
        if (found.length) present.value.add(item.id);
        else {
          await host.call(props.contribution.pluginId, 'action', { action: operation.action, payload: { ...operation.payload, resourceId: item.id, video } });
          // Desktop acknowledges the start; Android returns after import.
          const deadline = Date.now() + 10 * 60_000;
          while (seq === sequence && !disposed) {
            const saved = await libraryStatus([item.id]);
            if (seq !== sequence || disposed) break;
            if (saved.length) { present.value.add(item.id); break; }
            const updated = await host.call(props.contribution.pluginId, 'renderSearch', { ids: [item.id], searchId: searchId.value }) as SearchRefresh | null;
            const row = (Array.isArray(updated) ? updated : updated?.items)?.find(result => result.id === item.id);
            if (!row?.actions?.some(action => action.id === operation.action && action.downloadOptions === false)) {
              // Import could finish between the disk lookup and the status read.
              if ((await libraryStatus([item.id])).length) break;
              throw new Error(row?.status || 'Download did not reach the library');
            }
            if (Date.now() >= deadline) throw new Error('Still downloading; check the result before retrying');
            await new Promise(resolve => setTimeout(resolve, 750));
          }
        }
        if (seq === sequence) selected.value.delete(item.id);
      } catch (e) { failed.push(`${item.title}: ${String(e)}`); }
    }
    if (seq === sequence && !disposed) { error.value = failed.join('\n'); await checkLibrary(); }
  } finally { actionBusy.value = false; }
}
async function search(cursor: string | null = null, direction: 'new' | 'next' | 'previous' = 'new') {
  const query = props.query.trim(); if (!query || loading.value || providerPending.value || actionBusy.value) return;
  loadingDirection.value = direction;
  const seq = ++sequence; refreshSequence++; providerPending.value = false; loading.value = true; error.value = ''; submitted.value = query;
  searchId.value = undefined; items.value = []; batch.value = undefined;
  try {
    const result = await host.call(props.contribution.pluginId, 'search', { providerId: props.contribution.id, query, cursor, limit: 120 }) as SearchResult;
    if (seq === sequence && !disposed) {
      batch.value = result.batch;
      items.value = validateUiNode({ type: 'results', items: result.items }).items || [];
      providerPending.value = result.pending === true;
      searchId.value = result.searchId; error.value = result.error || '';
      if (direction === 'new') previousCursors.value = [];
      else if (direction === 'next') previousCursors.value.push(currentCursor.value);
      else previousCursors.value.pop();
      currentCursor.value = cursor; nextCursor.value = typeof result.nextCursor === 'string' ? result.nextCursor : null;
    }
  } catch (e) { if (seq === sequence && !disposed) { error.value = String(e); providerPending.value = false; } }
  finally { if (seq === sequence) loading.value = false; }
}
async function action(action: string, payload: Record<string, unknown> = {}, button?: string) {
  if (loading.value || actionBusy.value) return;
  actionBusy.value = true; pendingButton.value = button;
  try { error.value = ''; await host.call(props.contribution.pluginId, 'action', { action, payload }); }
  catch (e) { error.value = String(e); }
  finally { actionBusy.value = false; pendingButton.value = undefined; }
}
watch(() => props.query, () => { sequence++; refreshSequence++; searchId.value = undefined; providerPending.value = false; items.value = []; error.value = ''; submitted.value = ''; loading.value = false; currentCursor.value = null; nextCursor.value = null; previousCursors.value = []; });
watch(() => props.sharedRequest, request => {
  if (!request) return;
  emit('sharedSearchStarted');
  void search();
}, { immediate: true, flush: 'post' });
watch(host.revision, async () => {
  if (!items.value.length && !providerPending.value && !searchId.value) return;
  const seq = sequence; const refresh = ++refreshSequence;
  try {
    const updated = await host.call(props.contribution.pluginId, 'renderSearch', { ids: items.value.map(i => i.id), searchId: searchId.value }) as SearchRefresh | null;
    if (!updated || seq !== sequence || refresh !== refreshSequence || disposed) return;
    if (!Array.isArray(updated) && updated.searchId !== undefined && updated.searchId !== searchId.value) return;
    items.value = validateUiNode({ type: 'results', items: Array.isArray(updated) ? updated : updated.items }).items || [];
    providerPending.value = !Array.isArray(updated) && updated.pending === true;
    if (!Array.isArray(updated)) error.value = updated.error || '';
  } catch (e) {
    if (seq === sequence && refresh === refreshSequence && !disposed && providerPending.value) {
      providerPending.value = false; error.value = String(e);
    }
  }
});
onUnmounted(() => { disposed = true; sequence++; });
</script>
<template>
  <section class="plugin-search">
    <div class="section-head">
      <h2>{{ contribution.label }}</h2>
      <button class="control-button" :disabled="loading || providerPending || actionBusy || !query.trim()" :aria-busy="(loading || providerPending) && loadingDirection === 'new'" @click="search()">{{ (loading || providerPending) && loadingDirection === 'new' ? 'Searching…' : 'Search' }}</button>
    </div>
    <p v-if="error" role="alert">{{ error }}</p>
    <p v-else-if="!submitted">Press Search to query {{ contribution.label }}.</p>
    <p v-else-if="!loading && !providerPending && !items.length">No matches for “{{ submitted }}”.</p>
    <div v-if="playlist && items.length" class="plugin-search-pages">
      <button class="control-button" :disabled="checking || Boolean(statusError) || actionBusy || !available.length" @click="selected = new Set(available.map(item => item.id))">Select available</button>
      <ResponsivePopup anchored :open="batchMenuOpen" @close="closeBatchMenu">
        <template #trigger>
          <button type="button" class="control-button" :disabled="batchDisabled" :aria-expanded="batchMenuOpen" @click.stop="batchMenuOpen = !batchMenuOpen">
            Download selected <AppIcon name="dropdown" :size="12" />
          </button>
        </template>
        <button type="button" class="batch-format" @click="downloadSelected(false)">Audio (WebM/Opus)</button>
        <button type="button" class="batch-format" @click="downloadSelected(true)">Video (MP4)</button>
      </ResponsivePopup>
    </div>
    <p v-if="statusError" role="alert">Could not check library: {{ statusError }} <button class="control-button" @click="checkLibrary">Retry</button></p>
    <PluginTree :node="{ type: 'results', items: rows }" :fields="{}" :busy="loading || actionBusy || checking || Boolean(statusError)" :pending-button="pendingButton" :selection="playlist ? selected : undefined" :unavailable="unavailable" @action="action" @select="select" />
    <div v-if="previousCursors.length || nextCursor" class="plugin-search-pages">
      <button class="control-button" :disabled="loading || providerPending || actionBusy || !previousCursors.length" :aria-busy="(loading || providerPending) && loadingDirection === 'previous'" @click="search(previousCursors[previousCursors.length - 1] ?? null, 'previous')">Previous</button>
      <button class="control-button" :disabled="loading || providerPending || actionBusy || !nextCursor" :aria-busy="(loading || providerPending) && loadingDirection === 'next'" @click="search(nextCursor, 'next')">Next</button>
    </div>
  </section>
</template>
<style scoped>
.plugin-search h2 { font-size: var(--fs-h2); font-weight: 800; margin: 0; }
.plugin-search { margin-top: var(--space-2); }
.plugin-search .section-head { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-4); }
.plugin-search-pages { display: flex; justify-content: flex-end; gap: var(--space-3); margin-top: var(--space-3); }
.batch-format { display: block; width: 100%; min-height: 40px; text-align: left; background: none; border: none; color: var(--color-text); font-size: var(--fs-button); padding: var(--space-2) var(--space-4); cursor: pointer; }
.batch-format:hover, .batch-format:focus-visible { background: var(--color-surface-hover); }
.plugin-search > p { color: var(--color-text-muted); }
</style>
