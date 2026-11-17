<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { usePluginHost } from '../host';
import type { AndroidCatalogInstallResult, AndroidPluginDiscovery, CatalogEntry, PluginCatalogListing, PluginDownload, PluginPreview } from '../types';
import PluginMetadata from './PluginMetadata.vue';
import PluginImage from './PluginImage.vue';
import PluginCatalog from './PluginCatalog.vue';
import AppToast from '../../components/AppToast.vue';
import AppIcon from '../../components/AppIcon.vue';

defineProps<{ reviewOnly?: boolean }>();
const emit = defineEmits<{ done: []; busy: [value: boolean] }>();
const host = usePluginHost();
const error = ref('');
const busy = ref('');
watch(busy, value => emit('busy', Boolean(value)));
const catalogPendingId = computed(() => busy.value.match(/^(?:catalog|uninstall):(.+)$/)?.[1]);
const submissionUrl = 'https://github.com/NoName-Audio/player-plugins/blob/main/docs/catalog.md#publish';
async function openSubmission() {
  try { await openUrl(submissionUrl); }
  catch (e) { error.value = String(e); }
}
const url = ref('');
const preview = ref<PluginPreview>();
const previewCard = ref<HTMLElement>();
const android = ref(false);
const apkPlugins = ref<AndroidPluginDiscovery['plugins']>([]);
const apkWarnings = ref<string[]>([]);
const catalogStatus = ref('');
const apkFallback = ref<CatalogEntry>();
async function downloadApk() {
  if (busy.value || !apkFallback.value?.androidApkUrl) return;
  busy.value = 'apk-download';
  try { await openUrl(apkFallback.value.androidApkUrl); }
  catch (e) { error.value = String(e); }
  finally { busy.value = ''; }
}
onMounted(() => { android.value = /Android/i.test(navigator.userAgent); });
let content = '';
function resetPreview() {
  const token = preview.value?.token;
  preview.value = undefined; content = '';
  if (token) void invoke('plugins_discard_preview', { token }).catch(() => {});
}
let disposed = false;
onUnmounted(() => { disposed = true; resetPreview(); });
function showPreview(candidate: string, checked: PluginPreview) {
  if (disposed) {
    if (checked.token) void invoke('plugins_discard_preview', { token: checked.token }).catch(() => {});
    return;
  }
  resetPreview();
  content = candidate; preview.value = checked;
  void nextTick(() => previewCard.value?.scrollIntoView({ block: 'nearest' }));
}
async function scanApks() {
  if (busy.value) return;
  busy.value = 'scan'; error.value = '';
  try {
    const found = await invoke<AndroidPluginDiscovery>('plugins_android_discover');
    apkPlugins.value = found.plugins; apkWarnings.value = found.warnings;
  } catch (e) { error.value = String(e); }
  finally { busy.value = ''; }
}
function previewApk(plugin: AndroidPluginDiscovery['plugins'][number]) {
  showPreview(plugin.content, plugin);
}
async function load() {
  if (busy.value) return;
  busy.value = 'preview'; error.value = '';
  try {
    const path = await openFileDialog({ multiple: false, filters: [{ name: 'Player plugin', extensions: ['player-plugin', 'json'] }] });
    if (!path) return;
    resetPreview();
    const checked = await invoke<PluginPreview>('plugins_preview_path', { path });
    showPreview('', checked);
  } catch (e) { error.value = String(e); }
  finally { busy.value = ''; }
}
async function loadUrl() {
  if (busy.value || !url.value.trim()) return;
  busy.value = 'download'; error.value = ''; resetPreview();
  try {
    const downloaded = await invoke<PluginDownload>('plugins_download', { url: url.value.trim() });
    showPreview('', downloaded);
  } catch (e) { error.value = String(e); }
  finally { busy.value = ''; }
}
async function loadCatalogPlugin(entry: CatalogEntry) {
  if (busy.value) return;
  busy.value = `catalog:${entry.id}`; error.value = ''; resetPreview();
  catalogStatus.value = ''; apkFallback.value = undefined;
  try {
    if (android.value && entry.androidApkUrl) {
      if (!entry.version) {
        apkFallback.value = entry;
        catalogStatus.value = 'This catalog entry has no version to verify. Download the APK manually, then find its plugin.';
        return;
      }
      catalogStatus.value = 'Installing APK… Complete the Android installer to continue.';
      const result = await invoke<AndroidCatalogInstallResult>('plugins_android_install_catalog', {
        url: entry.androidApkUrl, expectedId: entry.id, expectedVersion: entry.version,
      });
      if (result.status === 'installed' && result.preview) {
        showPreview(result.preview.content, result.preview);
        catalogStatus.value = 'APK installed. Review the permissions to connect the plugin.';
      } else {
        apkFallback.value = entry;
        catalogStatus.value = result.message || (result.status === 'cancelled'
          ? 'APK installation cancelled.'
          : result.status === 'permission-denied'
            ? 'APK installation permission was denied.'
            : 'APK installation could not be completed.');
      }
      return;
    }

    let packageUrl = entry.installUrl || entry.repository;
    if (entry.installUrls) {
      const listing = await invoke<PluginCatalogListing>('plugins_catalog');
      const selectedUrl = entry.installUrls[listing.nativeTarget];
      if (!selectedUrl) throw new Error('No plugin package is available for this architecture');
      packageUrl = selectedUrl;
    }
    const downloaded = await invoke<PluginDownload>('plugins_download', {
      url: packageUrl, expectedId: entry.id,
    });
    showPreview('', downloaded);
  } catch (e) {
    error.value = String(e);
    if (android.value && entry.androidApkUrl) {
      apkFallback.value = entry;
      catalogStatus.value = 'APK installation could not be completed.';
    }
  }
  finally { busy.value = ''; }
}
defineExpose({ review: loadCatalogPlugin });
async function install() {
  if (!preview.value || busy.value) return;
  try {
    busy.value = 'install'; error.value = '';
    await host.install(content, preview.value.fingerprint, preview.value.source, preview.value.token, preview.value.manifest);
    resetPreview();
    emit('done');
  } catch (e) { error.value = String(e); }
  finally { busy.value = ''; }
}
async function uninstall(id: string) {
  if (busy.value) return;
  try {
    busy.value = `uninstall:${id}`; error.value = '';
    if (host.plugins.value.find(plugin => plugin.manifest.id === id)?.enabled) await host.setEnabled(id, false);
    await host.uninstall(id);
  }
  catch (e) { error.value = String(e); }
  finally { busy.value = ''; }
}
</script>

<template>
  <section class="plugin-manager">
    <AppToast :message="error" @dismiss="error = ''" />
    <div v-if="preview" ref="previewCard" class="plugin-card">
      <div class="plugin-heading">
        <PluginImage :manifest="preview.manifest" />
        <div><h4>{{ preview.manifest.name }} <small>{{ preview.manifest.version }}</small></h4><p>{{ preview.manifest.description }}</p></div>
      </div>
      <PluginMetadata :manifest="preview.manifest" />
      <p class="plugin-native-note">Native code runs inside Player with the app’s system access.</p>
      <p>Permissions: {{ preview.manifest.permissions.join(', ') || 'none' }}</p>
      <template v-if="preview.manifest.androidNative">
        <small class="fingerprint">Signing certificate SHA-256: {{ preview.manifest.androidNative.certificateSha256 }}</small>
        <small class="fingerprint">APK SHA-256: {{ preview.manifest.androidNative.apkSha256 }}</small>
      </template>
      <small class="fingerprint">BLAKE3: {{ preview.fingerprint }}</small>
      <div class="plugin-actions plugin-install-actions">
        <button class="control-button control-button-primary" :disabled="Boolean(busy)" :aria-busy="busy === 'install'" @click="install">{{ host.plugins.value.some(plugin => plugin.manifest.id === preview!.manifest.id) ? 'Update and grant these permissions' : 'Install and grant these permissions' }}</button>
        <button class="control-button" :disabled="Boolean(busy)" @click="resetPreview">Cancel</button>
      </div>
    </div>
    <PluginCatalog v-if="!reviewOnly" :busy="Boolean(busy)" :pending-id="catalogPendingId" :pending-action="busy.startsWith('uninstall:') ? 'uninstall' : 'review'" @review="loadCatalogPlugin" @uninstall="uninstall" @notify="error = $event" />
    <p v-if="catalogStatus" role="status">{{ catalogStatus }}</p>
    <div v-if="apkFallback" class="plugin-card">
      <p>Download and install {{ apkFallback.name }} manually, then find its APK plugin to review the connection.</p>
      <div class="plugin-actions">
        <button class="control-button" :disabled="Boolean(busy)" :aria-busy="busy === 'apk-download'" @click="downloadApk">Download APK</button>
        <button class="control-button" :disabled="Boolean(busy)" :aria-busy="busy === 'scan'" @click="scanApks">{{ busy === 'scan' ? 'Scanning…' : 'Find APK plugins' }}</button>
      </div>
    </div>
    <div v-if="!reviewOnly" class="plugin-catalog-submission">
      <a :href="submissionUrl" @click.prevent="openSubmission">Add your plugin <AppIcon name="external-link" :size="14" /></a>
    </div>
    <div v-if="!reviewOnly" class="plugin-install">
      <h4>Manual installation</h4>
      <template v-if="android">
        <p>Install the extension APK on Android, then find it here.</p>
        <button class="control-button" :disabled="Boolean(busy)" :aria-busy="busy === 'scan'" @click="scanApks">{{ busy === 'scan' ? 'Scanning…' : 'Find APK plugins' }}</button>
        <p v-for="warning in apkWarnings" :key="warning" class="error" role="alert">{{ warning }}</p>
        <div v-for="plugin in apkPlugins" :key="plugin.manifest.id" class="plugin-card">
          <div class="plugin-heading">
            <PluginImage :manifest="plugin.manifest" />
            <div><h4>{{ plugin.manifest.name }} <small>{{ plugin.manifest.version }}</small></h4><p>{{ plugin.manifest.description }}</p></div>
          </div>
          <button class="control-button" :disabled="Boolean(busy)" @click="previewApk(plugin)">Review connection</button>
        </div>
      </template>
      <form class="plugin-url" @submit.prevent="loadUrl">
        <label for="plugin-url">Public GitHub repository, plugin folder or package URL</label>
        <div class="plugin-url-row"><input id="plugin-url" v-model="url" class="form-input" type="url" placeholder="https://github.com/author/plugin" autocomplete="off" :disabled="Boolean(busy)" /><button class="control-button" :disabled="Boolean(busy) || !url.trim()" :aria-busy="busy === 'download'">{{ busy === 'download' ? 'Loading…' : 'Load from URL' }}</button></div>
      </form>
      <button class="control-button" :disabled="Boolean(busy)" :aria-busy="busy === 'preview'" @click="load">Or choose a .player-plugin package</button>
    </div>
  </section>
</template>

<style scoped src="./plugin-manager.css"></style>
