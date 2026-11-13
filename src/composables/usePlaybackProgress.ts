import { onUnmounted, ref, watch, type Ref } from 'vue';
import { createPlaybackProgressClock } from '../utils/playbackProgress';

/** One render clock shared by every seek surface; never writes transport state. */
export function usePlaybackProgress(options: {
  position: Ref<number>;
  duration: Ref<number>;
  playing: Ref<boolean>;
  preview: Ref<number | null>;
  reducedMotion: Ref<boolean>;
  identity: () => string;
}) {
  const clock = createPlaybackProgressClock();
  const renderedPosition = ref(options.position.value);
  let frame: number | null = null;
  let disposed = false;
  const canAnimate = () => !disposed && document.visibilityState === 'visible' && !options.reducedMotion.value
    && options.playing.value && options.preview.value === null;
  function draw() {
    frame = null;
    renderedPosition.value = clock.read(performance.now(), options.duration.value);
    if (canAnimate()) frame = requestAnimationFrame(draw);
  }
  function schedule() {
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
    if (canAnimate()) frame = requestAnimationFrame(draw);
  }
  function reconcile(position: number, playing: boolean) {
    clock.anchor(position, playing, performance.now());
    renderedPosition.value = clock.read(performance.now(), options.duration.value);
    schedule();
  }
  watch(options.playing, playing => {
    // Freeze exactly where the last rendered frame was, without another transport poll.
    reconcile(renderedPosition.value, playing);
  }, { flush: 'sync' });
  watch(options.identity, () => reconcile(options.position.value, false), { flush: 'sync' });
  watch([options.preview, options.reducedMotion], schedule, { flush: 'sync' });
  watch(options.duration, () => {
    renderedPosition.value = clock.read(performance.now(), options.duration.value);
  });
  document.addEventListener('visibilitychange', schedule);
  onUnmounted(() => {
    disposed = true;
    schedule();
    document.removeEventListener('visibilitychange', schedule);
  });
  return { renderedPosition, reconcile };
}
