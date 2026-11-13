import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import ts from 'typescript';
const source = readFileSync(new URL('../src/utils/playbackProgress.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 } }).outputText;
const { createPlaybackProgressClock } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);

test('fractional transport anchor advances between polls and pauses without creeping', () => {
  const clock = createPlaybackProgressClock();
  clock.anchor(12.125, true, 1000);
  assert.equal(clock.read(1125, 100), 12.25);
  clock.anchor(clock.read(1125, 100), false, 1125);
  assert.equal(clock.read(9000, 100), 12.25);
});
test('seek and authoritative corrections jump immediately, without tweening', () => {
  const clock = createPlaybackProgressClock();
  clock.anchor(10.25, true, 0);
  clock.anchor(80.75, true, 250);
  assert.equal(clock.read(250, 100), 80.75);
  assert.equal(clock.read(375, 100), 80.875);
  clock.anchor(79.5, false, 500);
  assert.equal(clock.read(900, 100), 79.5);
});
test('duration clamps, monotonic elapsed time and stalled polls are bounded', () => {
  const clock = createPlaybackProgressClock();
  clock.anchor(99.75, true, 1000);
  assert.equal(clock.read(1500, 100), 100);
  assert.equal(clock.read(900, 100), 99.75);
  clock.anchor(5, true, 1000);
  assert.equal(clock.read(10000, 100), 6);
  assert.equal(clock.read(10000, 0), 0);
  clock.anchor(Number.NaN, false, 0);
  assert.equal(clock.read(10, 100), 0);
});
