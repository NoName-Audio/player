# Component attachments for native plugins

## Status

Implemented. Existing plugin contributions (`page`, `settings`, `search`, `track-action`, `player-widget`) remain supported and unchanged. API 2 manifests without attachments retain their contract. Older strict hosts reject the optional `attachments` field; no API/ABI bump or legacy-path removal was made.

## Goal

Allow native plugins to render UI before or after an explicitly exposed Vue component without adding two plugin slots at every call site or manipulating the DOM outside Vue.

## Component integration

Provide a `withPluginAttachments` helper that wraps a component once and renders a Vue Fragment containing before attachments, the original component, and after attachments.

```ts
const PluginAlbumCard = withPluginAttachments(AlbumCard, {
  target: 'AlbumCard',
  context: props => ({ album: props.album }),
});
```

Call sites use the wrapped component normally:

```vue
<PluginAlbumCard :album="album" @open="openAlbum" />
```

The wrapper forwards props, attributes, slots and listeners without duplicating event handling. It adds no DOM container. Target names are explicit public identifiers, independent of filenames or inferred Vue component names. The initial integration candidate is the existing `AlbumCard`.

Vue lifecycle hooks can observe mounting and updates, but do not themselves provide managed before/after rendering. Do not use `element.before()` / `element.after()` or globally monkey-patch Vue component internals. A build-time transform could automate wrapping later, but is not part of this proposal.

## Plugin declaration

Add an optional `attachments` manifest section, independently of existing contributions:

```json
{
  "attachments": [
    {
      "id": "album-extra-info",
      "target": "AlbumCard",
      "position": "after",
      "view": "album-extra-info",
      "order": 100
    }
  ]
}
```

A registered target automatically exposes `before` and `after`. Attachments from enabled, supported, running plugins are sorted by order and then stable plugin/attachment identity. Unknown targets do not render. Optional fields require coordinated updates to host and SDK manifest validation; older strict parsers may reject them, so compatibility/version negotiation must be decided before implementation.

## Native rendering contract

Proposed invocation:

```json
{
  "method": "renderAttachment",
  "args": {
    "attachmentId": "album-extra-info",
    "viewId": "album-extra-info",
    "target": "AlbumCard",
    "instanceId": "host-generated-instance-id",
    "context": { "album": { "album": "Example" } }
  }
}
```

The plugin returns the existing validated declarative UI tree, or null when nothing applies to this instance. No plugin JavaScript, arbitrary HTML or Vue component instance is exposed.

Each mounted target instance receives a stable host-generated identity for its lifetime. Render requests and actions are scoped to plugin activation generation, attachment and target instance. Discard stale results after context changes, deactivation or unmount. Context is an explicit serializable projection, not arbitrary component props or internal state. Existing host permissions remain authoritative.

## Design and behavior constraints

- Preserve the original component's markup, styling, props, slots and event behavior.
- Before/after means siblings around the component's output, not insertion inside its root element.
- Extra siblings can affect flex/grid layout and structural CSS selectors even without a wrapper element. Review each exposed target's parent layout.
- Define attribute fallthrough and component-ref behavior explicitly; Fragment wrappers do not automatically preserve either.
- Transition, TransitionGroup, KeepAlive, Teleport and multi-root components require explicit support decisions and verification, not assumed transparency.
- Attachment rendering must not recursively expose the same target through its own UI renderer.
- Reuse existing UI validation, action routing and plugin invalidation where applicable; bound and coalesce renders for repeated cards rather than triggering unnecessary native calls.
- Do not intercept every component globally. Explicit registration defines the supported extension surface.

## Acceptance before shipping

1. Existing contributions work unchanged, including manifests without attachments.
2. One wrapped component registration enables before/after at all its wrapped call sites.
3. Multiple instances and plugins render deterministically; actions receive the correct context.
4. Updated context, invalidation, unmount and activation changes cannot apply stale output.
5. Original slots, attributes, listeners and interactions retain their behavior.
6. Actual desktop/mobile UI verification covers populated cards, absent attachments and layout changes caused by present attachments.

## Implementation and proof

`src/plugins/AlbumCard.ts` is the single registration used by ArtistView's existing
AlbumCard call site. `withPluginAttachments` forwards ordinary component props,
defaults, attrs, listeners and slots exactly once in a Fragment. Ref consumers
use the explicitly exposed `{ component }` child instance, not transparent
forwarding. Functional/async components, special Vue wrappers, multi-root targets,
and recursively wrapped targets are unsupported; only ordinary explicitly
registered single-root components are extension surfaces. PluginTree does not
instantiate registered targets. Attachment sections are independent flex/grid
siblings and may occupy additional layout space rather than altering card markup.

Enabled, supported, running attachments sort by order then plugin/attachment
identity. Host-generated instance IDs survive context changes. Generation-keyed
instances, synchronous context/revision invalidation, sequence guards and unmount
guards reject stale output and action completion. A four-call host queue bounds
native rendering and coalesces queued updates. Null responses create no UI.
Native scope validation checks declared attachment/view/target and activation;
the frontend owns mounted-instance provenance and projected context. Existing
permission checks remain authoritative. The SDK packager and Hello example
implement the same additive declaration and dispatch contract.

Verification performed:

- `pnpm exec vue-tsc --noEmit` passed.
- Frontend plugin regression and attachment queue tests: 14 passed.
- Rust host plugin tests (`cargo test --lib plugins:: --no-default-features`):
  36 passed, including native C ABI execution, manifest compatibility and scope
  checks. Existing unrelated audio/dead-code warnings remain.
  A subsequent native C ABI test dispatched the real Hello attachment and
  verified contextual UI plus null output through the compiled shared library.
- SDK and Hello tests (`cargo test -p player-plugin-sdk -p player-hello-plugin`):
  6 passed; null/dispatch/packager/example coverage included.
- Actual Chromium desktop and 390px mobile-width smoke used production wrapper,
  AlbumCard, PluginAttachment and PluginTree with two populated albums and a
  throwaway injected fixture host. No attachments produced two unchanged cards,
  no sections and no calls. Before/after produced four sections in sibling order.
  Distinct instances received correct contexts. Album and artist events fired
  once; attrs/defaults/slots were preserved in a generic wrapper probe. Input
  fields reached actions and delayed-action controls disabled. Context changes,
  generation replacement, unmount and null responses discarded stale/empty UI.
  Mobile document width remained 390px with four attachment sections.

Desktop baseline, desktop attachments and mobile screenshots were captured at
`/var/folders/k6/65nj89mj0jzctv_b9m8zq6bh0000gn/T/omp-sshots-15cb44813a74e4e5.webp`,
`/var/folders/k6/65nj89mj0jzctv_b9m8zq6bh0000gn/T/omp-sshots-15cb44945df4e4e6.webp`, and
`/var/folders/k6/65nj89mj0jzctv_b9m8zq6bh0000gn/T/omp-sshots-15cb44947834e4e7.webp`.
The smoke fixture was deleted and its server stopped. Browser proof exercised
fixture responses, not an installed native plugin or physical mobile device;
native behavior is covered by Rust contracts/ABI tests, not claimed device UI
verification. The spoof-payload fixture did not reach the observed payload, so
that run proves scope transmission, not adversarial collision preservation.

