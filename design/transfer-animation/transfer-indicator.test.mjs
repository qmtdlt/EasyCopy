import test from 'node:test';
import assert from 'node:assert/strict';
import { TransferIndicator } from './transfer-indicator.mjs';

function setup(options = {}) {
  const callbacks = new Map();
  const frames = [];
  let nextId = 0;
  const indicator = new TransferIndicator({
    draw: (frame) => frames.push(frame),
    requestFrame: (callback) => { callbacks.set(++nextId, callback); return nextId; },
    cancelFrame: (id) => callbacks.delete(id),
    ...options,
  });
  const step = (timestamp) => {
    const pending = [...callbacks.values()];
    callbacks.clear();
    pending.forEach((callback) => callback(timestamp));
    return frames.at(-1);
  };
  return { indicator, step, callbacks, frames };
}

test('instant completion is full immediately and stays opaque for at least 300ms', () => {
  const { indicator, step, callbacks } = setup();
  indicator.play(0);
  assert.equal(step(100).progress, 1);
  assert.equal(step(399).opacity, 1);
  assert.equal(indicator.state.phase, 'holding');
  assert.equal(step(400).fadeStartMs, 300);
  assert.equal(step(470).opacity, 0.5);
  assert.equal(step(540).phase, 'hidden');
  assert.equal(callbacks.size, 0);
});

test('short transfers reach their actual progress before the minimum hold expires', () => {
  const { indicator, step } = setup();
  indicator.play(40);
  assert.equal(step(0).progress, 0);
  assert.equal(step(20).progress, 0.5);
  assert.equal(step(40).phase, 'holding');
  assert.equal(step(299).opacity, 1);
  assert.equal(step(300).phase, 'fading');
});

test('long transfers do not receive another 300ms hold after completion', () => {
  const { indicator, step } = setup();
  indicator.play(1000);
  step(0);
  assert.equal(step(300).phase, 'transferring');
  assert.equal(step(500).progress, 0.5);
  assert.equal(step(1000).fadeStartMs, 1000);
  assert.equal(step(1140).phase, 'hidden');
});

test('delayed first paint does not consume the 300ms visibility budget', () => {
  const { indicator, step } = setup();
  indicator.play(0);
  assert.equal(step(5000).elapsedMs, 0);
  assert.equal(step(5299).phase, 'holding');
  assert.equal(step(5300).phase, 'fading');
});

test('replay cancels stale callbacks instead of hiding the newer transfer', () => {
  const { indicator, step, callbacks, frames } = setup();
  indicator.play(0);
  step(0);
  const stale = [...callbacks.values()][0];
  indicator.play(1000);
  const frameCount = frames.length;
  stale(500);
  assert.equal(frames.length, frameCount);
  assert.equal(step(500).progress, 0);
  assert.equal(step(800).phase, 'transferring');
});

test('reduced-motion fade still preserves the minimum visible time', () => {
  const { indicator, step } = setup({ fadeMs: 0 });
  indicator.play(0);
  step(0);
  assert.equal(step(299).opacity, 1);
  assert.equal(step(300).phase, 'hidden');
});
