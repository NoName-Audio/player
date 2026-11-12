<script setup lang="ts">
import { onUnmounted, ref, watch } from 'vue';
import { usePluginHost, validateUiNode } from '../host';
import type { UiAttachment, UiNode } from '../types';
import PluginTree from './PluginTree.vue';
const props = defineProps<{ attachment: UiAttachment; instanceId: string; context: Record<string, unknown> }>();
const host = usePluginHost();
const node = ref<UiNode>();
const fields = ref<Record<string, string | boolean>>({});
const busy = ref(false);
const pendingButton = ref<string>();
const error = ref('');
const queueKey = {};
let sequence = 0;
let disposed = false;
function scope() {
  const a = props.attachment;
  return { attachmentId: a.id, viewId: a.view, target: a.target, instanceId: props.instanceId, context: JSON.parse(JSON.stringify(props.context)) };
}
function current(seq: number, generation: number) {
  return !disposed && sequence === seq && host.attachments.value.some(a => a.key === props.attachment.key && a.generation === generation);
}
function defaults(n: UiNode) {
  if (n.type === 'input' && n.key && fields.value[n.key] === undefined) fields.value[n.key] = n.value ?? '';
  n.children?.forEach(defaults);
}
function render() {
  const seq = ++sequence;
  const a = props.attachment;
  const args = scope();
  node.value = undefined; error.value = ''; fields.value = {};
  host.attachmentQueue.schedule(queueKey, async () => {
    if (!current(seq, a.generation)) return;
    try {
      const result = await host.call(a.pluginId, 'renderAttachment', args);
      if (!current(seq, a.generation)) return;
      node.value = result === null ? undefined : validateUiNode(result);
      if (node.value) defaults(node.value);
    } catch (e) { if (current(seq, a.generation)) error.value = String(e); }
  });
}
async function action(action: string, payload = {}, button?: string) {
  if (busy.value || !node.value) return;
  const seq = sequence;
  const a = props.attachment;
  if (!current(seq, a.generation)) return;
  busy.value = true; pendingButton.value = button;
  try {
    // Plugin-controlled payload stays nested: it cannot override host-owned scope.
    await host.call(a.pluginId, 'action', { action, payload, fields: { ...fields.value }, ...scope() });
    if (current(seq, a.generation)) render();
  } catch (e) { if (current(seq, a.generation)) error.value = String(e); }
  finally { if (!disposed) { busy.value = false; pendingButton.value = undefined; } }
}
watch(() => [props.attachment, props.context, host.revision.value], render, { immediate: true, deep: true, flush: 'sync' });
onUnmounted(() => { disposed = true; sequence++; host.attachmentQueue.cancel(queueKey); });
</script>
<template>
  <section v-if="node || error" class="plugin-attachment" :data-plugin-attachment="attachment.key" :aria-busy="busy">
    <p v-if="error" role="alert">{{ error }}</p>
    <PluginTree v-if="node" :node="node" :fields="fields" :busy="busy" :pending-button="pendingButton" @action="action" />
  </section>
</template>
<style scoped>
.plugin-attachment { min-width: 0; max-width: 100%; align-self: stretch; overflow-wrap: anywhere; }
</style>
