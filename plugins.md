# Native plugins

The system supports native plugins only: desktop shared libraries, Android APKs,
and independently installed Rust packages. Current contracts: Plugin API 2, C ABI 1, catalog schema 2.

Plugin author guide: [player-plugins documentation](https://github.com/NoName-Audio/player-plugins/blob/main/docs/README.md).
Packages and SDK: [NoName-Audio/player-plugins](https://github.com/NoName-Audio/player-plugins).

Installed plugins show a primary **Update** action when the fetched catalog has a
strictly newer semantic version supported on the current platform. Refresh the
catalog to check again. Updating opens the same permission and fingerprint review
as installation; approval replaces the package while retaining settings, secrets,
and the previous enabled state. **Uninstall** remains a separate action. Missing
or invalid versions do not offer updates, and build metadata alone is not newer.

On Android, select **Install** in the catalog to download the APK inside Player
and open Android’s system confirmation. If requested, allow Player to install
unknown apps and return to Player. Android installation does not grant plugin
capabilities: Player then shows the installed APK’s actual permissions and signing
fingerprints for explicit review before connection.

If installation is cancelled, permission is denied, or downloading fails, use the
shown **Download APK** action, install it manually, and select **Find APK plugins**.
Player does not repeatedly open permission settings. Downloads are HTTPS-only,
limited to 64 MiB, validated before installation, and removed from private cache
after the operation. Updates must retain the installed signing certificate;
already-loaded native code still requires restarting Player after an APK update.

Search result `coverHash` values use the host cover store. The plugin result
renderer owns its thumbnail dimensions (36 px desktop, 32 px narrow screens);
it must not depend on scoped track-row styles in the application shell.
Result items may set `coverLoading: true` while artwork discovery or transfer is
pending, including before `coverHash` exists. The renderer uses the player's
existing striped image loader; plugins must clear this flag on success, missing
artwork, or failure rather than infer loading from a missing hash.

Search providers return `{ items, nextCursor, pending?: boolean, searchId?: string,
error?: string }`. A provider that completes in the initial call can omit
`pending`; asynchronous providers set it to `true` and publish host revisions as
results change. Give each submission a distinct `searchId`: the host forwards it
alongside current result IDs to `renderSearch`, including when the list is empty.
Refresh returns either the legacy item array or `{ items, pending?: boolean,
searchId?: string, error?: string }`. Echo the submission ID, not a global latest
query. Return `pending: false` on completion; failures can return `error` with an
empty list or fail the call. Never insert pending, count, empty, or error markers
as result items. The existing Search button stays busy while pending, even with
no items. Result actions remain usable after the initial call, and changing the
query discards stale responses.

## Receiving Android shares

Declare the `library.import` permission and a `share-handler` contribution, e.g.
`{"kind":"share-handler","id":"receive","label":"Receive shares"}`.

- `canShare` receives untrusted metadata: `{"url":"…"}` or `{"name":"…","mime":"…"}`. Return JSON `true` for supported items. No host calls, UI, or side effects.
- `share` receives the same metadata. Return `{"handled":true}` to consume the item (including when the user cancels plugin UI), or `{"handled":false}` to decline.
- To open the plugin’s search tab, return `{"handled":true,"searchQuery":"…"}`. Player runs the query in that tab.

Player calls a sole match directly. With multiple matches, it offers only matching plugins and, for files, direct library import. Cancel does nothing. Files fall back to library import if no plugin matches or the sole match declines; an explicit plugin choice never falls back.

- During file `share`, `host.call("share.read", {"offset":0,"length":65536})` returns `{"data":"…"}` in base64. Read 1–65536 bytes per call, advance by the decoded length, and stop on empty data. No file path or URI is exposed.

## Component attachments

Optional manifest `attachments` render native declarative UI before/after the
public `AlbumCard` target. Existing API 2 contributions and manifests without
attachments are unchanged. API and C ABI versions remain 2 and 1 respectively.
Older strict hosts reject manifests containing `attachments`; distribute a
manifest without that field when supporting those hosts. This is not a fallback
to executable plugin JavaScript.

The generated [attachment target reference](plugins/docs/attachment-targets.md)
lists the supported targets and their context types. See the
[plugin author guide](https://github.com/NoName-Audio/player-plugins/blob/main/docs/README.md)
for the native SDK and manifest contract.

### Maintaining the target reference

From the Player repository root, run `pnpm attachments:docs` to write
`plugins/docs/attachment-targets.md`, or use
`pnpm attachments:docs --output /tmp/attachment-targets.md` to override the output.
The generator reads explicit `withPluginAttachments` registrations through the
TypeScript AST and resolves their projected context types from the TypeScript
source; registrations and types, not a manually maintained target list, are the
source of truth. Keep the registration JSDoc useful to plugin authors.

Normal application builds never generate this reference or modify the plugins
submodule. CI generates the `attachment-targets-preview` artifact for review.
After a Player release is published, release automation proposes the generated
reference in a cross-repository pull request to `NoName-Audio/player-plugins`
from the `automation/attachment-targets` branch; publication is reviewed there
rather than pushed directly to its default branch.

Configure the Player repository variable `PLUGINS_DOCS_APP_ID`
(`vars.PLUGINS_DOCS_APP_ID`) and secret `PLUGINS_DOCS_APP_PRIVATE_KEY`
(`secrets.PLUGINS_DOCS_APP_PRIVATE_KEY`) for this automation. Install the GitHub
App only on `NoName-Audio/player-plugins`, granting **Contents: write** and
**Pull requests: write** permissions.

### Runtime behavior

`withPluginAttachments` registers the ordinary single-root AlbumCard once; all
ArtistView card instances use it. Props/defaults, attrs, slots and listeners are
forwarded once to the original component, with no extra surrounding element.
Component refs explicitly expose `{ component }`, the original public instance;
they are not transparent aliases. Functional/async components, special Vue
wrappers (Transition, TransitionGroup, KeepAlive, Teleport), multi-root targets,
wrapping an already wrapped target, and embedding wrapped targets in the plugin
UI renderer are unsupported. Only explicitly registered ordinary targets belong
to this extension surface; PluginTree does not render AlbumCard recursively.

Before/after UI is sibling content and therefore occupies its own flex/grid item.
In ArtistView's horizontal/expanded album list it consumes additional space;
it does not become content inside the card. Plugins should keep attachment UI
compact. Empty/null UI creates no element. Rendering is globally limited to four
native attachment calls, coalesces queued updates per attachment instance, and
discards obsolete context/revision/generation/unmount responses. Actions retain
the host-generated instance/context scope separately from plugin payload fields.

Status popups keep track details within the available width, wrapping long names
and placing result actions below the details on desktop and mobile. This layout
is scoped to status menus; ordinary plugin search results keep their row layout.
Success status icons use the theme accent, while error/inactive icons use muted
text. The popup's Close action follows its content.
On mobile, player status icons form a vertical tray immediately above Activity
at the bottom-right safe-area-aware dock. Desktop keeps the horizontal footer
tray. Moving between layouts relocates the same plugin panels rather than
mounting a second set or duplicating their polling.
Each status icon, interactive or static, has its own Activity-style surface and
border while preserving its status color and 32px footprint.

Refreshing the local library, including after saving a plugin search result,
keeps provider search panels mounted. Their results, pagination, and pending
actions remain intact without automatically issuing another provider search.

