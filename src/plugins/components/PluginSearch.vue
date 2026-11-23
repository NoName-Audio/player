<script setup lang="ts">
import { onUnmounted, ref, watch } from 'vue';
import { usePluginHost, validateUiNode } from '../host';
import type { SearchRefresh, SearchResult, SearchItem, UiContribution } from '../types';
import PluginTree from './PluginTree.vue';
const props = defineProps<{ contribution: UiContribution; query: string }>();
const host = usePluginHost(); const items = ref<SearchItem[]>([]); const error = ref(''); const loading = ref(false); const submitted = ref('');
const loadingDirection = ref<'new' | 'next' | 'previous'>('new');
const providerPending = ref(false);
const searchId = ref<string>();
let refreshSequence = 0;
const actionBusy = ref(false); const pendingButton = ref<string>();
let sequence = 0; let disposed = false;
const currentCursor = ref<string | null>(null); const nextCursor = ref<string | null>(null); const previousCursors = ref<Array<string | null>>([]);
async function search(cursor: string | null = null, direction: 'new' | 'next' | 'previous' = 'new') {
  const query = props.query.trim(); if (!query || loading.value || providerPending.value || actionBusy.value) return;
  loadingDirection.value = direction;
  const seq = ++sequence; refreshSequence++; providerPending.value = false; loading.value = true; error.value = ''; submitted.value = query;
  searchId.value = undefined; items.value = [];
  try {
    const result = await host.call(props.contribution.pluginId, 'search', { providerId: props.contribution.id, query, cursor, limit: 120 }) as SearchResult;
    if (seq === sequence && !disposed) {
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
    <PluginTree :node="{ type: 'results', items }" :fields="{}" :busy="loading || actionBusy" :pending-button="pendingButton" @action="action" />
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
.plugin-search > p { color: var(--color-text-muted); }
</style>
