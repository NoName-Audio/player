<script setup lang="ts">
import AppIcon from './AppIcon.vue';

defineProps<{
  canNavBack: boolean;
  canNavForward: boolean;
  title?: string;
  subtitle?: string;
  searchExpanded?: boolean;
}>();

const emit = defineEmits<{
  openMenu: [];
  navBack: [];
  navForward: [];
}>();
</script>

<template>
  <header class="topbar" :class="{ 'has-page-heading': title, 'search-expanded': searchExpanded }">
    <button class="burger-btn" @click="emit('openMenu')" aria-label="Menu">
      <AppIcon name="menu" :size="24" />
    </button>
    <div class="nav-arrows">
      <button class="arrow-btn" aria-label="Back" :disabled="!canNavBack" @click="emit('navBack')"><AppIcon name="history-back" :size="22" /></button>
      <button class="arrow-btn" aria-label="Forward" :disabled="!canNavForward" @click="emit('navForward')"><AppIcon name="history-forward" :size="22" /></button>
    </div>
    <div v-if="title" class="page-heading">
      <h1 :title="title"><slot name="title">{{ title }}</slot></h1>
      <p v-if="subtitle" :title="subtitle">{{ subtitle }}</p>
    </div>
    <div v-if="$slots.actions" class="topbar-actions"><slot name="actions" /></div>
  </header>
</template>

<style scoped>
.topbar {
  container: topbar / inline-size;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-5);
  padding: var(--space-4) var(--page-padding);
  padding-top: calc(var(--space-4) + env(safe-area-inset-top));
  flex-shrink: 0;
}

.nav-arrows {
  display: flex;
  gap: var(--control-gap);
  flex-shrink: 0;
}
.page-heading { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.topbar-actions { display: flex; align-items: center; justify-content: flex-end; gap: var(--control-gap); margin-left: auto; flex-shrink: 0; }
.page-heading h1 { color: var(--color-text); font-size: 24px; font-weight: 800; letter-spacing: -.5px; line-height: 1.25; margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.page-heading p { color: var(--color-text-muted); font-size: 13px; line-height: 1.5; margin: var(--space-1) 0 0; display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; }
.arrow-btn {
  width: 32px;
  height: 32px;
  border-radius: 50%;
  background: color-mix(in srgb, var(--color-shadow) 45%, transparent);
  border: none;
  color: var(--color-text);
  font-size: 22px;
  line-height: 1;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
}
.arrow-btn:hover:not(:disabled) { background: color-mix(in srgb, var(--color-shadow) 70%, transparent); }
.arrow-btn:disabled { opacity: .4; cursor: default; }

.burger-btn {
  display: none;
}

@media (max-width: 768px) {
  .burger-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    color: var(--color-text);
    cursor: pointer;
    padding: var(--space-2);
    flex-shrink: 0;
  }
  .topbar {
    padding: var(--space-3) var(--page-padding);
    padding-top: calc(var(--space-3) + env(safe-area-inset-top));
    gap: var(--space-2);
  }
  .page-heading h1 { font-size: 20px; }
  .page-heading p { font-size: 12px; }
  .topbar.has-page-heading { display: grid; grid-template-columns: auto auto minmax(0, 1fr) auto; }
  .topbar.has-page-heading .topbar-actions { grid-column: 4; justify-self: end; }
  .topbar:not(.has-page-heading) { flex-wrap: wrap; }
}
@container topbar (width < 480px) {
  .topbar.search-expanded > :not(.topbar-actions) { display: none; }
  .topbar.search-expanded .topbar-actions { grid-column: 1 / -1; width: 100%; }
}
</style>
