import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const source = readFileSync(new URL('../src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const parsed = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const names = ['createEmptySpectrum', 'normalizeSpectrum', 'resetSpectrum', 'refreshSpectrum', 'startSpectrumPolling', 'stopSpectrumPolling'];
const functions = parsed.statements.filter(node => ts.isFunctionDeclaration(node) && names.includes(node.name?.text)).map(node => node.getText(parsed)).join('\n');
const compiled = ts.transpileModule(functions, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;

function harness() {
  let finish;
  const state = {
    SPECTRUM_SEGMENT_COUNT: 32, spectrumLevels: { value: [] }, spectrumActive: { value: true },
    spectrumPollVersion: 0, spectrumPollPending: false, spectrumPollTimer: null,
    calls: 0, timers: new Set(),
    invoke: () => { state.calls++; return new Promise(resolve => { finish = resolve; }); },
    setInterval: fn => { state.timers.add(fn); return fn; },
    clearInterval: fn => state.timers.delete(fn),
  };
  vm.createContext(state);
  vm.runInContext(compiled, state);
  return { state, finish: value => finish(value) };
}

test('invalidated in-flight FFT cannot restore stale bars or overlap another request', async () => {
  const { state, finish } = harness();
  state.startSpectrumPolling();
  state.stopSpectrumPolling();
  state.startSpectrumPolling();
  assert.equal(state.calls, 1);
  finish([1]);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(state.spectrumLevels.value.every(value => value === 0), true);
  assert.equal(state.spectrumPollPending, false);
  const next = state.refreshSpectrum();
  assert.equal(state.calls, 2);
  finish([0.7, NaN, Infinity, -1, 2]);
  await next;
  assert.deepEqual(Array.from(state.spectrumLevels.value.slice(0, 5)), [0.7, 0, 0, 0, 1]);
});

test('inactive output does not poll and stopping resets bars and removes timer', async () => {
  const { state, finish } = harness();
  state.spectrumActive.value = false;
  state.startSpectrumPolling();
  await state.refreshSpectrum();
  assert.equal(state.calls, 0);
  state.spectrumActive.value = true;
  state.startSpectrumPolling();
  finish([0.8]);
  await new Promise(resolve => setImmediate(resolve));
  state.spectrumActive.value = false;
  state.stopSpectrumPolling();
  assert.equal(state.timers.size, 0);
  assert.equal(state.spectrumLevels.value.every(value => value === 0), true);
  await state.refreshSpectrum();
  assert.equal(state.calls, 1);
});
