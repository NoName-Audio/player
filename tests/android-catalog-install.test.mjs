import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

// Execute the component's real orchestration, with only IPC and reactive refs replaced.
function harness(invoke, android = true) {
  const source = readFileSync(new URL('../src/plugins/components/PluginInstaller.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
  const parsed = ts.createSourceFile('installer.ts', source, ts.ScriptTarget.Latest, true);
  const fn = parsed.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === 'loadCatalogPlugin');
  const code = ts.transpileModule(`export ${fn.getText(parsed)}`, {compilerOptions:{module:ts.ModuleKind.CommonJS}}).outputText;
  const state = {busy:{value:''},error:{value:''},catalogStatus:{value:''},apkFallback:{value:undefined},preview:undefined};
  const exports = {};
  new Function('exports','invoke','android','busy','error','catalogStatus','apkFallback','resetPreview','showPreview',code)(exports, invoke, {value:android}, state.busy,state.error,state.catalogStatus,state.apkFallback,()=>{state.preview=undefined;},(content,preview)=>{state.preview={content,preview};});
  return {...state, load:exports.loadCatalogPlugin, getPreview:()=>state.preview};
}
const entry = {id:'example.plugin',version:'1.0.4',androidApkUrl:'https://example.org/plugin.apk',repository:'https://example.org/repo'};
test('Android installation presents actual permission preview without connecting capabilities', async()=>{
  const preview={content:'validated APK',manifest:{id:entry.id,permissions:['native.android']},fingerprint:'pinned'};
  const calls=[]; const h=harness(async(command,args)=>{calls.push([command,args]);return {status:'installed',preview};});
  await h.load(entry);
  assert.deepEqual(calls,[['plugins_android_install_catalog',{url:entry.androidApkUrl,expectedId:entry.id,expectedVersion:entry.version}]]);
  assert.deepEqual(h.getPreview(),{content:preview.content,preview});
  assert.equal(h.apkFallback.value,undefined); assert.equal(h.busy.value,'');
});
for(const status of ['cancelled','permission-denied','failed']) test(`${status} exposes manual fallback without retry`,async()=>{
  let calls=0; const h=harness(async()=>{calls++;return {status,message:'Use manual installation'};});
  await h.load(entry); assert.equal(calls,1); assert.equal(h.apkFallback.value,entry);
  assert.equal(h.getPreview(),undefined); assert.equal(h.catalogStatus.value,'Use manual installation'); assert.equal(h.busy.value,'');
});
test('download errors expose fallback and duplicate clicks do not start another install',async()=>{
  let reject; let calls=0; const h=harness(()=>{calls++;return new Promise((_,fail)=>{reject=fail;});});
  const pending=h.load(entry); await h.load(entry); assert.equal(calls,1);
  reject(new Error('HTTPS failure')); await pending;
  assert.equal(h.apkFallback.value,entry); assert.match(h.error.value,/HTTPS failure/); assert.equal(h.busy.value,'');
});
test('missing version never starts Android installer; desktop retains download review',async()=>{
  const missing=harness(()=>{throw new Error('must not invoke');}); await missing.load({...entry,version:undefined}); assert.ok(missing.apkFallback.value);
  const calls=[]; const preview={content:'desktop',manifest:{id:entry.id},fingerprint:'desktop-pin'};
  const desktop=harness(async(...args)=>{calls.push(args);return preview;},false); await desktop.load(entry);
  assert.deepEqual(calls,[['plugins_download',{url:entry.repository,expectedId:entry.id}]]); assert.equal(desktop.getPreview().content,'desktop');
});
