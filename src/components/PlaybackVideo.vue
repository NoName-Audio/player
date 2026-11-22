<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';

const props = defineProps<{ sourceId: string }>();
const emit = defineEmits<{ 'audio-only': [sourceId: string] }>();
const url = computed(() => convertFileSrc(props.sourceId, 'player-video'));
const video = ref<HTMLVideoElement>();
const failed = ref(false);
const canvas = ref<HTMLCanvasElement>();
const legacy = ref(false);
const native = ref(false);
let hasDrawnFrame = false;
async function captureFrame(): Promise<number[]> {
  const element = canvas.value ?? video.value;
  const width = canvas.value ? (hasDrawnFrame ? canvas.value.width : 0) : (video.value?.videoWidth ?? 0);
  const height = canvas.value?.height ?? video.value?.videoHeight ?? 0;
  if (!element || !width || !height || failed.value || (element instanceof HTMLVideoElement && element.readyState < 2)) {
    throw new Error('No video frame is available yet.');
  }
  const scale = Math.min(1, 1280 / width, 720 / height);
  const frame = document.createElement('canvas');
  frame.width = Math.max(1, Math.round(width * scale));
  frame.height = Math.max(1, Math.round(height * scale));
  const context = frame.getContext('2d');
  if (!context) throw new Error('Could not capture the video frame.');
  context.drawImage(element, 0, 0, frame.width, frame.height);
  const blob = await new Promise<Blob>((resolve, reject) => frame.toBlob(
    (result) => result ? resolve(result) : reject(new Error('Could not encode the video frame.')),
    'image/png',
  ));
  return Array.from(new Uint8Array(await blob.arrayBuffer()));
}
defineExpose({ captureFrame });
void invoke<boolean>('native_video_enabled').then((enabled) => {
  native.value = enabled;
  legacy.value = !enabled;
}).catch(() => { failed.value = true; });
watch(canvas, (element, _, cleanup) => {
  if (!element) return;
  const context = element.getContext('2d');
  if (!context) { failed.value = true; return; }
  let active = true;
  let request = 0;
  cleanup(() => { active = false; cancelAnimationFrame(request); });
  async function draw() {
    try {
      const data = await invoke<ArrayBuffer | number[]>('native_video_frame', { sourceId: props.sourceId });
      if (!active) return;
      const bytes = Array.isArray(data) ? Uint8Array.from(data).buffer : data;
      if (bytes.byteLength >= 8) {
        const header = new DataView(bytes);
        const width = header.getUint32(0, true);
        const height = header.getUint32(4, true);
        if (element!.width !== width) element!.width = width;
        if (element!.height !== height) element!.height = height;
        context!.putImageData(new ImageData(new Uint8ClampedArray(bytes, 8), width, height), 0, 0);
        hasDrawnFrame = true;
      }
      request = requestAnimationFrame(draw);
    } catch {
      if (active) failed.value = true;
    }
  }
  request = requestAnimationFrame(draw);
});

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
  <canvas v-if="!failed && native" ref="canvas" />
  <video v-else-if="!failed && legacy" ref="video" :src="url" crossorigin="anonymous" muted playsinline preload="auto"
    @loadedmetadata="!($event.target as HTMLVideoElement).videoWidth && emit('audio-only', sourceId)"
    @error="failed = true" />
  <p v-else-if="failed" role="status">Video unavailable. Audio playback is unaffected.</p>
</template>

<style scoped>
video, canvas { display: block; width: 100%; height: 100%; background: black; }
canvas { object-fit: inherit; }
</style>
