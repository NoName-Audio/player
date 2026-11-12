import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, symlinkSync, rmSync } from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { extractAttachments, renderAttachments } from './generate-attachment-docs.mjs';

function fixture(t, registration) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'attachment-docs-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(path.join(root, 'src/plugins'), { recursive: true });
  symlinkSync(path.resolve('node_modules'), path.join(root, 'node_modules'));
  writeFileSync(path.join(root, 'tsconfig.json'), JSON.stringify({ compilerOptions: { strict: true, target: 'ES2020', module: 'ESNext', moduleResolution: 'bundler', skipLibCheck: true }, include: ['src/**/*'] }));
  writeFileSync(path.join(root, 'src/plugins/withPluginAttachments.ts'), `import type { Component } from 'vue';
export function withPluginAttachments<C extends Component>(component: C, options: { target: string; context: (props: C extends new (...args: never[]) => { $props: infer P } ? P : never) => Record<string, unknown> }) { return { component, options }; }`);
  writeFileSync(path.join(root, 'src/Card.vue'), `<script setup lang="ts">\ninterface Album {\n/** Album display title. */\ntitle: string; artist: { name: string }; tracks: { id: number }[] }\ndefineProps<{ album: Album }>();\n</script><template><div /></template>`);
  writeFileSync(path.join(root, 'src/registration.ts'), `import Card from './Card.vue';\nimport { withPluginAttachments as attach } from './plugins/withPluginAttachments';\n${registration}`);
  return root;
}

test('resolves Vue SFC props, imported/local aliases, nested fields and JSDoc', t => {
  const root = fixture(t, `const alias = attach;\n/** Album card attachment. */\nexport const card = alias(Card, { target: 'Card', context: props => ({ album: props.album }) });`);
  const targets = extractAttachments(root);
  assert.equal(targets.length, 1);
  assert.equal(targets[0].description, 'Album card attachment.');
  assert.equal(targets[0].fields.find(f => f.name === 'album.title').type, 'string');
  assert.match(targets[0].fields.find(f => f.name === 'album.title').description, /Album display title/);
  assert.equal(targets[0].fields.find(f => f.name === 'album.artist.name').type, 'string');
  assert.equal(targets[0].fields.find(f => f.name === 'album.tracks[].id').type, 'number');
  assert.equal(renderAttachments(targets, { version: '1', commit: 'abc', status: 'clean' }), renderAttachments(targets, { version: '1', commit: 'abc', status: 'clean' }));
});
test('rejects duplicate IDs with source locations', t => {
  const root = fixture(t, `attach(Card, { target: 'Card', context: p => ({ album: p.album }) });\nattach(Card, { target: 'Card', context: p => ({ album: p.album }) });`);
  assert.throws(() => extractAttachments(root), /registration.ts:\d+:\d+: Duplicate attachment target ID Card/);
});
test('rejects dynamic IDs and nonstatic options', t => {
  const root = fixture(t, `const target = 'Card'; attach(Card, { target, context: p => ({ album: p.album }) });`);
  assert.throws(() => extractAttachments(root), /explicit static property assignments/);
});
test('rejects unresolved context types', t => {
  const root = fixture(t, `attach(Card, { target: 'Card', context: () => ({ value: undefined as any }) });`);
  assert.throws(() => extractAttachments(root), /Context field value has unresolved any/);
});
