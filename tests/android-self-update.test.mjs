import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

function harness(invoke) {
  const source = readFileSync(new URL('../src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
  const parsed = ts.createSourceFile('app.ts', source, ts.ScriptTarget.Latest, true);
  const names = ['applyAndroidUpdateStatus', 'refreshAndroidUpdateStatus', 'cancelAboutUpdate', 'doAboutUpdate'];
  const functions = parsed.statements.filter(node => ts.isFunctionDeclaration(node) && names.includes(node.name?.text)).map(node => node.getText(parsed)).join('\n');
  const refs = Object.fromEntries(Object.entries({aboutAndroidUpdate:null,aboutUpdating:false,aboutUpdateProgress:null,aboutUpdateError:'',aboutUpdateDone:'',aboutInfo:{platform:'android'},aboutUpdateStatus:{asset_url:'https://example.org/player.apk'},supportsTauriUpdater:false}).map(([name,value])=>[name,{value}]));
  const code = ts.transpileModule(`${functions}\nreturn {doAboutUpdate,refreshAndroidUpdateStatus,cancelAboutUpdate,applyAndroidUpdateStatus};`, {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  const timers=[];
  const api = new Function('invoke',...Object.keys(refs),'setTimeout','clearTimeout',`let aboutUpdatePollTimer=null,aboutUpdatePollPending=false,aboutUpdateDisposed=false,aboutUpdateStarting=false;\n${code}`)(invoke,...Object.values(refs),fn=>{timers.push(fn);return timers.length;},()=>{});
  return {...api,...refs,timers};
}

test('Android start polls native status and uses actual bytes without claiming success',async()=>{
  const calls=[];
  const h=harness(async(command)=>{calls.push(command);return command==='about_update_status'?{status:calls.includes('about_do_update')?'downloading':'idle',downloaded:250,total:1000}:'native status';});
  await h.doAboutUpdate();
  assert.deepEqual(calls,['about_update_status','about_do_update','about_update_status']);
  assert.equal(h.aboutUpdateProgress.value,25);
  assert.equal(h.aboutUpdateDone.value,'');
  assert.equal(h.aboutUpdating.value,true);
  assert.equal(h.timers.length,1);
});

test('restored permission and installer states prevent duplicate starts',async()=>{
  for(const status of ['permission','installing','verifying']) {
    const calls=[];
    const h=harness(async(command)=>{calls.push(command);return {status,downloaded:1000,total:1000};});
    await h.refreshAndroidUpdateStatus();
    await h.doAboutUpdate();
    assert.deepEqual(calls,['about_update_status']);
    assert.equal(h.aboutUpdateDone.value,'');
    assert.equal(h.aboutUpdateProgress.value,null);
  }
});

test('native cancellation and errors release the update button',async()=>{
  const h=harness(async()=>({status:'cancelled',downloaded:10,total:100}));
  await h.cancelAboutUpdate();
  assert.equal(h.aboutUpdating.value,false);
  assert.equal(h.aboutUpdateDone.value,'Update cancelled.');
  h.applyAndroidUpdateStatus({status:'error',downloaded:10,total:100,error:'Signature mismatch'});
  assert.equal(h.aboutUpdateError.value,'Signature mismatch');
  assert.equal(h.aboutUpdating.value,false);
});

test('failed starts remain visible after an idle status response',async()=>{
  const h=harness(async(command)=>{if(command==='about_do_update') throw new Error('Network unavailable');return {status:'idle',downloaded:0,total:0};});
  await h.doAboutUpdate();
  assert.match(h.aboutUpdateError.value,/Network unavailable/);
  assert.equal(h.aboutUpdating.value,false);
});
