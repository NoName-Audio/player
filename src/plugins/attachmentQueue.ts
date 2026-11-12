/** At most four native attachment calls at once; one queued job per mounted attachment. */
export function createAttachmentQueue(limit = 4) {
  let active = 0;
  const pending = new Map<object, () => Promise<void>>();
  const running = new Set<object>();
  function pump() {
    while (active < limit && pending.size) {
      const entry = [...pending.entries()].find(([key]) => !running.has(key));
      if (!entry) return;
      const [key, job] = entry;
      pending.delete(key); active++; running.add(key);
      void job().finally(() => { active--; running.delete(key); pump(); });
    }
  }
  return {
    schedule(key: object, job: () => Promise<void>) { pending.set(key, job); queueMicrotask(pump); },
    cancel(key: object) { pending.delete(key); },
  };
}
