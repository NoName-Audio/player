<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
const props = defineProps<{ progress: number; waveform: number[] | null; disabled?: boolean }>();
const root = ref<HTMLElement>();
const width = ref(768);
let observer: ResizeObserver | undefined;
onMounted(() => {
  observer = new ResizeObserver(([entry]) => { width.value = entry.contentRect.width; });
  if (root.value) observer.observe(root.value);
});
onUnmounted(() => observer?.disconnect());
const path = computed(() => {
  if (!props.waveform?.length) return 'M0 9.5H768';
  const samples = props.waveform;
  const count = Math.min(samples.length, Math.max(1, Math.floor(width.value / 3)));
  const step = 768 / count;
  return Array.from({ length: count }, (_, index) => {
    const start = index * samples.length / count;
    const end = (index + 1) * samples.length / count;
    let energy = 0;
    for (let bin = Math.floor(start); bin < Math.ceil(end); bin++) {
      const overlap = Math.min(end, bin + 1) - Math.max(start, bin);
      energy += Math.min(255, Math.max(0, samples[bin])) ** 2 * overlap;
    }
    const amplitude = Math.sqrt(energy / (end - start));
    const height = Math.max(1, amplitude / 255 * 17);
    return `M${(index + 0.5) * step} ${9.5 - height / 2}v${height}`;
  }).join('');
});
const progress = computed(() => Math.max(0, Math.min(100, props.progress || 0)));
</script>

<template>
  <div ref="root" class="seek-spectrum" :class="{ disabled }" aria-hidden="true" :style="{ '--seek-progress': `${progress}%` }">
    <svg class="bars" viewBox="0 0 768 19" preserveAspectRatio="none" focusable="false"><path :d="path" /></svg>
    <svg class="bars played" viewBox="0 0 768 19" preserveAspectRatio="none" focusable="false"><path :d="path" /></svg>
    <span class="position" />
  </div>
</template>

<style scoped>
.seek-spectrum { position: relative; width: 100%; height: 19px; pointer-events: none; }
.bars { position: absolute; inset: 0; width: 100%; height: 100%; fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; color: var(--color-border-strong); }
.bars path { vector-effect: non-scaling-stroke; }
.played { color: var(--color-accent); clip-path: inset(0 calc(100% - var(--seek-progress)) 0 0); }
.position { position: absolute; left: clamp(2px, var(--seek-progress), calc(100% - 2px)); top: 2px; width: 2px; height: 15px; border-radius: 2px; background: var(--color-text); transform: translateX(-50%); box-shadow: 0 0 0 1px var(--color-bg); }
:global([role="slider"]:focus-visible) .bars:not(.played) { color: var(--color-text-muted); }
:global([role="slider"]:focus-visible) .position { background: var(--color-accent-hover); }
.disabled { opacity: .45; }
.disabled .position { display: none; }
</style>
