<script setup lang="ts">
import { ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { Marked } from 'marked';
import appIconUrl from '/app-icon.svg';
import contributorSnapshot from '../generated/contributors.json';
import type { AboutInfo, AboutUpdateStatus, AndroidUpdateStatus, Contributor } from '../types/settings';
import AppIcon from './AppIcon.vue';

const markdown = new Marked({ gfm: true, breaks: true, renderer: { html: () => '' } });
const contributors: Contributor[] = contributorSnapshot.contributors;
const failedAvatars = ref(new Set<string>());
const assetsBaseUrl = import.meta.env.BASE_URL;

type ReleaseNote = { tag_name: string; body: string | null; published_at: string | null };
type ChangelogEntry = ReleaseNote & { previewHtml: string; notesHtml: string };
const releases = ref<ChangelogEntry[]>([]);
const nextPage = ref<number | null>(1);
const releaseLoading = ref(false);
const releaseError = ref(false);
const releaseSentinel = ref<HTMLElement>();
const releaseMarkdown = new Marked({ gfm: true, breaks: true, renderer: {
  html: () => '',
  link({ tokens }) { return this.parser.parseInline(tokens); },
  image: () => '',
} });

function formatRelease(release: ReleaseNote): ChangelogEntry {
  const tokens = releaseMarkdown.lexer(release.body?.trim() ?? '');
  const start = tokens.findIndex(token => token.type === 'heading' && /^highlights$/i.test(token.text.trim()));
  if (start < 0) return { ...release, previewHtml: '', notesHtml: releaseMarkdown.parser(tokens) };
  const boundary = tokens.findIndex((token, index) => index > start && (
    token.type === 'heading' || (token.type === 'paragraph' && /^Full changelog:/i.test(token.text))
  ));
  const end = boundary < 0 ? tokens.length : boundary;
  return {
    ...release,
    previewHtml: releaseMarkdown.parser(tokens.slice(start + 1, end)),
    notesHtml: releaseMarkdown.parser([...tokens.slice(0, start), ...tokens.slice(end)]),
  };
}

async function loadReleases() {
  const page = nextPage.value;
  if (page === null || releaseLoading.value) return;
  releaseLoading.value = true;
  releaseError.value = false;
  try {
    const result = await invoke<{ releases: ReleaseNote[]; next_page: number | null }>('about_release_notes', { page });
    const seen = new Set(releases.value.map(release => release.tag_name));
    releases.value.push(...result.releases.filter(release => !seen.has(release.tag_name)).map(formatRelease));
    nextPage.value = result.next_page;
  } catch {
    releaseError.value = true;
  } finally {
    releaseLoading.value = false;
  }
}

watch(releaseSentinel, (element, _, onCleanup) => {
  if (!element || typeof IntersectionObserver === 'undefined') return;
  const observer = new IntersectionObserver(([entry]) => {
    if (props.autoCheck && entry?.isIntersecting) void loadReleases();
  }, { rootMargin: '200px 0px' });
  observer.observe(element);
  onCleanup(() => observer.disconnect());
}, { flush: 'post' });

const props = defineProps<{
  info: AboutInfo | null;
  loading: boolean;
  error: string;
  status: AboutUpdateStatus | null;
  checking: boolean;
  updating: boolean;
  progress: number | null;
  desktopReady: boolean;
  updateError: string;
  updateDone: string;
  androidUpdate: AndroidUpdateStatus | null;
  autoCheck: boolean;
}>();
const emit = defineEmits<{ 'update:autoCheck': [value: boolean]; check: []; update: []; cancelUpdate: []; openRelease: []; openRepository: []; retry: []; openExternal: [url: string] }>();
function openNoteLink(event: MouseEvent) {
  const link = (event.target as HTMLElement).closest('a');
  if (!link) return;
  event.preventDefault();
  const url = link.getAttribute('href');
  if (url && /^https?:\/\//i.test(url)) emit('openExternal', url);
}
</script>

<template>
  <div class="about-panel">
    <p v-if="loading && !info" role="status">Loading app information...</p>
    <div v-else-if="error && !info" class="error" role="alert">{{ error }} <button class="control-button" @click="emit('retry')">Try again</button></div>
    <template v-else>
      <div class="app-identity">
        <div class="app-summary">
          <img class="app-mark" :src="appIconUrl" alt="" width="64" height="64" />
          <div><h3>Player</h3><p>Local-first music player with native playback, smart playlists, Soulseek search, and LAN sync.</p></div>
        </div>
        <button v-if="info?.release_repo" class="control-button text-link repository-link" @click="emit('openRepository')">GitHub repository <AppIcon name="external-link" /></button>
      </div>
      <div class="info-row"><span>Version</span><strong>{{ info?.display_version || 'Unavailable' }}</strong></div>
      <div class="info-row"><span>Platform</span><span>{{ info?.platform || 'Unavailable' }}</span></div>
      <div class="update-card">
        <div><h3>{{ status?.has_update ? 'An update is available' : 'App updates' }}</h3>
          <p v-if="status?.has_update">Version {{ status.latest_version }} is ready.</p>
          <p v-else-if="status">{{ status.latest_version ? 'You have the latest release.' : status.message }}</p>
          <p v-else>Check for the latest version of Player.</p>
        </div>
        <button v-if="status?.has_update || updating" class="control-button control-button-primary progress-button" :disabled="updating" :aria-busy="updating" @click="emit('update')">
          <span v-if="updating && progress !== null && (!androidUpdate || androidUpdate.status === 'downloading')" class="button-progress-fill" aria-hidden="true" :style="{ width: `${Math.min(100, Math.max(0, progress))}%` }" />
          <span class="button-progress-label">{{ updating ? (androidUpdate?.status === 'permission' ? 'Permission required' : androidUpdate?.status === 'installing' ? 'Installer opened' : androidUpdate?.status === 'verifying' ? 'Verifying APK...' : progress !== null ? `Downloading ${progress}%` : 'Downloading...') : desktopReady || status?.asset_url ? 'Update now' : 'View release' }}</span>
        </button>
        <button class="control-button" v-else :disabled="checking" :aria-busy="checking" @click="emit('check')">{{ checking ? 'Checking...' : 'Check for updates' }}</button>
        <button v-if="updating && androidUpdate && androidUpdate.status !== 'installing'" class="control-button" @click="emit('cancelUpdate')">Cancel update</button>
        <label class="check-option"><input type="checkbox" :checked="autoCheck" @change="emit('update:autoCheck', ($event.target as HTMLInputElement).checked)" /> Check for updates automatically</label>
      </div>
      <p v-if="androidUpdate?.status === 'downloading'" role="status">Downloaded {{ androidUpdate.downloaded.toLocaleString() }}{{ androidUpdate.total > 0 ? ` of ${androidUpdate.total.toLocaleString()}` : '' }} bytes.</p>
      <p v-else-if="androidUpdate?.status === 'verifying'" role="status">Verifying the downloaded APK before installation.</p>
      <p v-else-if="androidUpdate?.status === 'permission'" role="status">Allow Player to install unknown apps in Android settings, then return here. Installation resumes automatically.</p>
      <p v-else-if="androidUpdate?.status === 'installing'" role="status">Continue in Android's package installer. Opening the installer does not confirm that the update was installed.</p>
      <p v-if="updateError" class="error" role="alert">{{ updateError }}</p>
      <p v-if="updateDone" role="status">{{ updateDone }}</p>
      <div v-if="status?.release_url" class="links">
        <button class="control-button text-link" @click="emit('openRelease')">Release page <AppIcon name="external-link" /></button>
      </div>
      <div v-if="contributors.length" class="contributors" aria-labelledby="contributors-heading">
        <h3 id="contributors-heading">Contributors</h3>
        <p>The people building Player.</p>
        <ul class="contributors-list">
          <li v-for="contributor in contributors" :key="contributor.login">
            <a class="contributor-link" :href="contributor.profileUrl" target="_blank" rel="noopener noreferrer" @click.prevent="emit('openExternal', contributor.profileUrl)">
              <span class="contributor-avatar" aria-hidden="true">
                <img v-if="contributor.avatarPath && !failedAvatars.has(contributor.login)" :src="`${assetsBaseUrl}${contributor.avatarPath}`" alt="" width="40" height="40" loading="lazy" decoding="async" @error="failedAvatars.add(contributor.login)" />
                <template v-else>{{ contributor.login[0].toUpperCase() }}</template>
              </span>
              <span class="contributor-info"><strong>{{ contributor.login }}</strong><span>GitHub profile</span></span>
              <AppIcon class="contributor-arrow" name="external-link" />
            </a>
          </li>
        </ul>
      </div>
      <div v-if="status?.has_update && status.release_notes" class="release-notes">
        <h3>Release notes</h3>
        <div class="release-notes-content" @click="openNoteLink" v-html="markdown.parse(status.release_notes)" />
      </div>
      <section class="changelog" aria-labelledby="changelog-heading">
        <h3 id="changelog-heading">Changelog</h3>
        <article v-for="release in releases" :key="release.tag_name" class="changelog-release release-notes">
          <h4>{{ release.tag_name }}</h4>
          <time v-if="release.published_at" :datetime="release.published_at">{{ new Date(release.published_at).toLocaleDateString() }}</time>
          <div v-if="release.previewHtml" class="release-notes-content" v-html="release.previewHtml" />
          <details v-if="release.notesHtml">
            <summary class="control-button changelog-toggle">{{ release.previewHtml ? 'More changes' : 'Release notes' }}</summary>
            <div class="release-notes-content" v-html="release.notesHtml" />
          </details>
          <p v-else-if="!release.previewHtml">No release notes.</p>
        </article>
        <p v-if="releaseLoading" role="status">Loading releases…</p>
        <p v-else-if="nextPage === null && !releases.length">No published releases found.</p>
        <template v-else-if="nextPage !== null">
          <p v-if="releaseError" role="alert">Could not load release history.</p>
          <button type="button" class="control-button" @click="loadReleases">{{ releaseError ? 'Try again' : releases.length ? 'Load more releases' : 'Load releases' }}</button>
          <div v-if="autoCheck && !releaseError" :key="nextPage" ref="releaseSentinel" aria-hidden="true" />
        </template>
      </section>
      <details v-if="info?.changelog.length" class="changelog">
        <summary class="control-button changelog-toggle">Recent build commits</summary>
        <ul><li v-for="entry in info.changelog" :key="entry.short_hash">{{ entry.subject }}</li></ul>
      </details>
      <footer class="about-footer">
        <button type="button" class="control-button text-link" @click="emit('openExternal', 'https://github.com/' + (info?.release_repo || 'NoName-Audio/player') + '/blob/HEAD/public/licenses/third-party.txt')">Licenses <AppIcon name="external-link" :size="14" /></button>
      </footer>
    </template>
  </div>
</template>

<style scoped>
.about-panel { display: flex; flex-direction: column; gap: var(--section-gap); color: var(--color-text-muted); font-size: 14px; }
.app-identity { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-4); }
.app-summary { display: flex; align-items: center; gap: var(--space-4); flex: 1 1 280px; min-width: 0; }
.app-summary > div { min-width: 0; overflow-wrap: anywhere; }
.app-mark { display: block; width: 64px; height: 64px; flex-shrink: 0; object-fit: contain; }
.repository-link { margin-left: auto; flex-shrink: 0; white-space: nowrap; }
h3 { color: var(--color-text); font-size: 16px; margin: 0 0 var(--space-1); }
.app-identity h3 { font-size: 24px; }
p { line-height: 1.6; margin: 0; }
.info-row { display: flex; flex-wrap: wrap; justify-content: space-between; gap: var(--space-4); padding-bottom: var(--space-4); border-bottom: 1px solid var(--color-border); }
.info-row > * { min-width: 0; overflow-wrap: anywhere; }
strong { color: var(--color-text); }
.update-card {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  align-items: center;
  gap: var(--space-4);
}
.update-card > .check-option { flex-basis: 100%; }
.changelog-toggle { display: inline-flex; align-items: center; gap: var(--control-gap); list-style: none; }
.changelog-toggle::-webkit-details-marker { display: none; }
.changelog-toggle:is(:hover, :active) {
  background: color-mix(in srgb, var(--color-text) 6%, transparent);
  border-color: var(--color-text-faint);
}
.changelog-toggle::after {
  content: '';
  width: 6px;
  height: 6px;
  border: solid currentColor;
  border-width: 0 1.5px 1.5px 0;
  transform: translateY(-2px) rotate(45deg);
}
details[open] > .changelog-toggle::after { transform: translateY(2px) rotate(225deg); }
.contributor-link {
  border-radius: 8px;
}
.contributor-link:focus-visible { outline: 2px solid var(--color-accent); outline-offset: 3px; }
.error { color: var(--color-danger-text); overflow-wrap: anywhere; }
.links { display: flex; flex-wrap: wrap; gap: var(--control-gap); }
.about-footer { display: flex; padding-top: var(--space-4); border-top: 1px solid color-mix(in srgb, var(--color-text) 8%, transparent); }
.about-footer .text-link { font-size: 12px; font-weight: 400; color: var(--color-text-muted); }
.text-link { display: inline-flex; align-items: center; gap: var(--control-gap); min-height: 40px; border: none; padding: 0; color: var(--color-text-muted); border-radius: 2px; }
.text-link:enabled:is(:hover, :active) { background: transparent; color: var(--color-text); text-decoration: underline; }
.contributors-list { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(200px, 100%), 1fr)); gap: var(--space-3); list-style: none; padding: 0; margin: var(--space-4) 0 0; }
.contributor-link { display: flex; align-items: center; gap: var(--space-3); padding: var(--panel-padding); color: var(--color-text-muted); text-decoration: none; }
.contributor-link:is(:hover, :active) { color: var(--color-text); }
.contributor-avatar { display: grid; place-items: center; width: 40px; height: 40px; border-radius: 50%; background: color-mix(in srgb, var(--color-accent) 15%, var(--color-surface)); color: var(--color-accent-text); font-size: 16px; font-weight: 700; flex-shrink: 0; }
.contributor-avatar img { display: block; width: 100%; height: 100%; border-radius: inherit; object-fit: cover; }
.contributor-info { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; overflow-wrap: anywhere; }
.contributor-info strong { font-size: 14px; }
.contributor-info > span { font-size: 12px; }
.contributor-arrow { margin-left: auto; }
.changelog-release { border-top: 1px solid var(--color-border); padding: var(--space-4) 0; }
.changelog-release h4 { margin: 0; }
.changelog-release time { color: var(--color-text-muted); }
.changelog ul { padding-left: var(--space-6); }
.changelog li { padding: var(--space-2) 0; line-height: 1.5; overflow-wrap: anywhere; }
.release-notes { line-height: 1.7; overflow-wrap: anywhere; }
.release-notes-content { margin-top: var(--space-4); }
.release-notes-content :deep(p),
.release-notes-content :deep(ul),
.release-notes-content :deep(ol),
.release-notes-content :deep(blockquote),
.release-notes-content :deep(pre) { margin: 0 0 var(--space-4); }
.release-notes-content :deep(ul),
.release-notes-content :deep(ol) { padding-left: var(--space-6); }
.release-notes-content :deep(ul) { list-style-type: disc; }
.release-notes-content :deep(ol) { list-style-type: decimal; }
.release-notes-content :deep(li + li) { margin-top: var(--space-2); }
.release-notes-content :deep(li > ul),
.release-notes-content :deep(li > ol) { margin-top: var(--space-2); margin-bottom: 0; }
.release-notes-content :deep(h1),
.release-notes-content :deep(h2),
.release-notes-content :deep(h3),
.release-notes-content :deep(h4),
.release-notes-content :deep(h5),
.release-notes-content :deep(h6) { margin: var(--space-6) 0 var(--space-3); color: var(--color-text); line-height: 1.4; }
.release-notes-content :deep(blockquote) { padding-left: var(--space-4); border-left: 2px solid var(--color-border); }
.release-notes-content :deep(pre) { padding: var(--space-3); overflow-x: auto; }
.release-notes-content :deep(> :first-child) { margin-top: 0; }
.release-notes-content :deep(> :last-child) { margin-bottom: 0; }
.release-notes :deep(a) { color: var(--color-accent-text); }
</style>
