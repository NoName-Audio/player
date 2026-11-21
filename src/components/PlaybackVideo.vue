<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';

const props = defineProps<{ sourceId: string }>();
const url = computed(() => convertFileSrc(props.sourceId, 'player-video'));
const video = ref<HTMLVideoElement>();
const failed = ref(false);

// The element never outputs audio. Reconcile against consumed PCM, not the
// seek bar's interpolated position or the video element's clock.
watch(video, (current, _, cleanup) => {
  if (!current) return;
  const element = current;
  let active = true;
  let timer: ReturnType<typeof setTimeout>;
  cleanup(() => { active = false; clearTimeout(timer); element.pause(); });
  let previous = -1;
  let lastSample = performance.now();
  let lastAdvance = lastSample;
  let lastCorrection = -Infinity;
  let lastRateUpdate = -Infinity;
  let playPending = false;
  let seekPending = true;
  async function update() {
    try {
      const state = await invoke<{ source_id: string | null; position: number; playing: boolean }>('playback_status');
      if (!active) return;
      if (state.source_id !== props.sourceId || element.error) { element.pause(); return; }
      const now = performance.now();
      const delta = state.position - previous;
      seekPending ||= previous >= 0 && (delta < -0.04 || delta > (now - lastSample) / 1000 + 0.4);
      if (delta !== 0 || !state.playing) lastAdvance = now;
      previous = state.position;
      lastSample = now;
      if (element.readyState < 1) return;

      // Audio position advances in output-buffer steps, not on every poll.
      const moving = state.playing && now - lastAdvance < 600;
      const target = Math.min(state.position, element.duration || state.position);
      const drift = target - element.currentTime;
      if (!moving) element.pause();
      // Let the decoder finish buffering before a drift correction. Seeking
      // while it is starved can discard the work needed to show the next frame.
      if (!element.seeking && (seekPending || (!moving && Math.abs(drift) > 0.08)
        || (moving && element.readyState >= HTMLMediaElement.HAVE_FUTURE_DATA
          && Math.abs(drift) > 1 && now - lastCorrection >= 2000))) {
        element.currentTime = target;
        lastCorrection = now;
        seekPending = false;
      }
      // Correct ordinary drift by changing video speed, not by flushing its decoder.
      if (!moving || now - lastRateUpdate >= 400) {
        const rate = moving && Math.abs(drift) > 0.08
          ? Math.max(0.85, Math.min(1.15, 1 + drift * 0.8)) : 1;
        if (element.playbackRate !== rate) element.playbackRate = rate;
        lastRateUpdate = now;
      }
      if (moving && !element.seeking && element.paused && !playPending) {
        playPending = true;
        void element.play().catch(() => {}).finally(() => { playPending = false; });
      }
    } finally {
      if (active) timer = setTimeout(() => void update().catch(() => {}), 80);
    }
  }
  void update().catch(() => {});
});
</script>

<template>
  <video v-if="!failed" ref="video" :src="url" muted playsinline preload="auto"
    @loadedmetadata="failed = !($event.target as HTMLVideoElement).videoWidth"
    @error="failed = true" />
  <p v-else role="status">Video unavailable. Audio playback is unaffected.</p>
</template>

<style scoped>
video { display: block; width: 100%; max-height: 45dvh; background: black; }
</style>
