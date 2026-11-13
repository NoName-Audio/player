// Render-only clock: transport positions remain authoritative and are never advanced here.
export function createPlaybackProgressClock() {
  let position = 0;
  let anchoredAt = 0;
  let playing = false;
  return {
    anchor(value: number, running: boolean, now: number) {
      position = Number.isFinite(value) ? Math.max(0, value) : 0;
      playing = running;
      anchoredAt = now;
    },
    read(now: number, duration: number) {
      // Stop extrapolating if status delivery stalls; never invent indefinite playback.
      const elapsed = playing ? Math.min(1, Math.max(0, (now - anchoredAt) / 1000)) : 0;
      return Math.max(0, Math.min(Math.max(0, duration), position + elapsed));
    },
  };
}
