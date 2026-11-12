# Native plugins

The system supports native plugins only: desktop shared libraries, Android APKs,
and independently installed Rust packages. Current contracts: Plugin API 2, C ABI 1, catalog schema 2.

Plugin author guide: [player-plugins documentation](https://github.com/NoName-Audio/player-plugins/blob/main/docs/README.md).
Packages and SDK: [NoName-Audio/player-plugins](https://github.com/NoName-Audio/player-plugins).

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

