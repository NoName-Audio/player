import type { Track } from '../types';
export interface Contribution {
  id: string; kind: 'page' | 'settings' | 'search' | 'track-action' | 'player-widget' | 'share-handler';
  label: string; description: string; icon?: string; order: number; slot?: 'player.actions' | 'player.status';
  /** Pages appear in the sidebar by default; false keeps only their route. */
  showInMenu?: boolean;
}
export interface Attachment {
  id: string; target: string; position: 'before' | 'after'; view: string; order: number;
}
export interface UiAttachment extends Attachment { pluginId: string; key: string; generation: number }
export interface PluginManifest {
  id: string; name: string; version: string; apiVersion: number; description: string;
  image?: string | null; pollIntervalMs?: number;
  platforms: string[]; permissions: string[]; contributions: Contribution[];
  attachments?: Attachment[];
  author?: string | { name: string; url?: string | null } | null;
  homepage?: string | null; repository?: string | null;
  androidNative?: { packageName: string; entryClass: string; certificateSha256: string; apkSha256: string } | null;
}
export interface InstallSource { url: string; repository?: string | null; revision?: string | null }
export interface PluginPreview { manifest: PluginManifest; fingerprint: string; source?: InstallSource; token?: string }
export interface PluginDownload extends PluginPreview { token: string; source: InstallSource }
export interface CatalogEntry {
  id: string; name: string; description: string; repository: string; installUrl?: string; androidApkUrl?: string;
  installUrls?: Record<string, string>;
  addedAt: string; updatedAt?: string; version?: string; platforms: string[]; tags: string[];
  author?: PluginManifest['author']; image?: string;
}
export interface PluginCatalogListing {
  catalog: { schemaVersion: number; updatedAt: string; plugins: CatalogEntry[] };
  platform: string;
  nativeTarget: string;
}
export interface AndroidPluginDiscovery { supported: boolean; plugins: Array<PluginPreview & { content: string }>; warnings: string[] }
export interface AndroidCatalogInstallResult {
  status: 'installed' | 'cancelled' | 'permission-denied' | 'failed';
  message?: string;
  preview?: PluginPreview & { content: string };
}
export interface PluginInfo {
  manifest: PluginManifest; supported: boolean; enabled: boolean;
  generation: number; fingerprint: string;
  installSource?: InstallSource | null;
}
export interface ResourceIdentity {
  pluginId: string; resourceId: string; title: string; artist: string; album: string;
  durationSeconds: number | null; coverHash: string | null;
}
export interface PlaybackSnapshot {
  title: string; artist: string; album: string; position: number; duration: number;
  playing: boolean; resource?: ResourceIdentity | null;
}
export interface PluginSource { sourceToken: string; resource: ResourceIdentity }
export interface ResultAction { id: string; label: string; disabled?: boolean; downloadOptions?: boolean }
export interface SearchItem {
  id: string; title: string; subtitle?: string; coverHash?: string | null; coverLoading?: boolean;
  status?: string; actions?: ResultAction[];
  progress?: number | null;
}
/** Async providers publish completion through renderSearch after a host revision. */
export interface SearchResult { items: SearchItem[]; nextCursor: string | null; pending?: boolean; searchId?: string; error?: string }
export type SearchRefresh = SearchItem[] | { items: SearchItem[]; pending?: boolean; searchId?: string; error?: string };
export interface UiNode {
  type: 'group' | 'text' | 'input' | 'button' | 'toggle' | 'results' | 'status' | 'account';
  /** Status children open a responsive popup; title defaults to the manifest name. */
  title?: string; children?: UiNode[]; text?: string; tone?: string;
  avatarUrl?: string;
  layout?: 'cards' | 'card';
  key?: string; label?: string; value?: string | boolean; inputType?: 'text' | 'password' | 'text-file';
  action?: string; payload?: Record<string, unknown>; disabled?: boolean; items?: SearchItem[];
}
export interface PluginEvent { pluginId: string; generation: number; revision: number; topic: string; payload: unknown }
export interface HostOptions {
  snapshot(): PlaybackSnapshot;
  transport(action: 'pause' | 'resume' | 'stop' | 'seek', position?: number): Promise<void>;
  playSource(plugin: PluginInfo, source: PluginSource): Promise<void>;
  adopt(pluginId: string, resourceId: string, track: Track): Promise<void>;
  navigate(route: string, context?: Record<string, unknown>): void;
  openSharedSearch(pluginId: string, providerId: string, query: string): void;
}
export interface UiContribution extends Contribution { pluginId: string; key: string }
