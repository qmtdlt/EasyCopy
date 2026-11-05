import { test } from 'node:test'
import assert from 'node:assert/strict'
import { TransferIndicator } from '../src/transfer-indicator.mjs'

function harness(options = {}) {
  let time = 0, next = 1
  const queue = new Map(), states = [], hidden = []
  const indicator = new TransferIndicator({
    now: () => time,
    requestFrame: callback => { const id = next++; queue.set(id, callback); return id },
    cancelFrame: id => queue.delete(id),
    draw: state => states.push(state),
    onHidden: id => hidden.push(id),
    ...options,
  })
  return { indicator, states, hidden, queue,
    frame(at) { time = at; const callbacks = [...queue.values()]; queue.clear(); callbacks.forEach(callback => callback(at)) },
    time(at) { time = at },
    last: () => states.at(-1),
  }
}
const event = (phase, extra = {}) => ({ taskId: 'a', direction: 'upload', phase, receivedBytes: 0, totalBytes: null, error: null, ...extra })

test('cancelled transfer fades and ignores late progress', () => {
  const h = harness()
  h.indicator.accept(event('start')); h.frame(0)
  h.time(400); h.indicator.accept(event('cancelled'))
  assert.equal(h.indicator.accept(event('transferring', { receivedBytes: 5 })), false)
  h.frame(540)
  assert.deepEqual(h.hidden, ['a'])
})

test('instant complete stays 300ms from its first rendered frame, then fades for 140ms', () => {
  const h = harness()
  h.indicator.accept(event('start'))
  h.indicator.accept(event('complete', { receivedBytes: 42, totalBytes: 42 }))
  h.frame(1000)
  assert.equal(h.last().progress, 1)
  assert.equal(h.last().opacity, 1)
  h.frame(1299)
  assert.equal(h.last().opacity, 1)
  h.frame(1300)
  assert.equal(h.last().opacity, 1)
  h.frame(1370)
  assert.equal(h.last().opacity, .5)
  h.frame(1440)
  assert.deepEqual(h.hidden, ['a'])
  assert.equal(h.last().opacity, 0)
})

test('long transfers fade immediately on verified completion without another 300ms hold', () => {
  const h = harness()
  h.indicator.accept(event('start')); h.frame(0)
  h.time(1800)
  h.indicator.accept(event('transferring', { receivedBytes: 100, totalBytes: 100 })); h.frame(1800)
  assert.equal(h.last().progress, 1)
  assert.deepEqual(h.hidden, [])
  h.time(2000); h.indicator.accept(event('complete', { receivedBytes: 100, totalBytes: 100 }))
  h.frame(2070)
  assert.equal(h.last().opacity, .5)
  assert.equal(h.last().fadeStartMs, 2000)
  h.frame(2140)
  assert.deepEqual(h.hidden, ['a'])
})

test('preparing and verification never manufacture received byte progress or completion', () => {
  const h = harness()
  h.indicator.accept(event('preparing')); h.frame(100)
  assert.equal(h.last().progress, 0)
  h.frame(10000)
  assert.equal(h.last().progress, 0)
  h.indicator.accept(event('transferring', { receivedBytes: 30, totalBytes: 200 })); h.frame(10001)
  assert.equal(h.last().progress, .15)
  h.indicator.accept(event('verifying', { receivedBytes: 200, totalBytes: 200 })); h.frame(12000)
  assert.equal(h.last().progress, 1)
  assert.equal(h.last().finished, false)
  assert.deepEqual(h.hidden, [])
})

test('failed transfer keeps actual progress and does not become success', () => {
  const h = harness()
  h.indicator.accept(event('start')); h.frame(0)
  h.time(30)
  h.indicator.accept(event('failed', { receivedBytes: 40, totalBytes: 100, error: 'checksum mismatch' }))
  h.frame(50)
  assert.equal(h.last().phase, 'failed')
  assert.equal(h.last().progress, .4)
  assert.equal(h.last().error, 'checksum mismatch')
  h.frame(440)
  assert.deepEqual(h.hidden, ['a'])
})

test('new task invalidates old callbacks and all subsequent old task events', () => {
  const h = harness()
  h.indicator.accept(event('start')); h.frame(0)
  h.time(20); h.indicator.accept(event('complete'))
  const oldCallback = [...h.queue.values()][0]
  h.indicator.accept(event('start', { taskId: 'b', direction: 'download' }))
  oldCallback(1000)
  assert.deepEqual(h.hidden, [])
  assert.equal(h.indicator.accept(event('complete')), false)
  assert.equal(h.indicator.accept(event('start')), false)
  h.frame(1000)
  assert.equal(h.last().taskId, 'b')
  assert.equal(h.last().direction, 'download')
  assert.equal(h.last().elapsedMs, 0)
  assert.deepEqual(h.hidden, [])
})

test('unknown late progress cannot supersede an active task', () => {
  const h = harness()
  h.indicator.accept(event('start', { taskId: 'new' })); h.frame(0)
  assert.equal(h.indicator.accept(event('complete', { taskId: 'missed-old-start' })), false)
  assert.equal(h.last().taskId, 'new')
})

test('duplicate start does not reset visible timer or transmitted bytes', () => {
  const h = harness()
  h.indicator.accept(event('start')); h.frame(0)
  h.indicator.accept(event('transferring', { receivedBytes: 50, totalBytes: 100 })); h.frame(100)
  h.time(200); h.indicator.accept(event('start')); h.frame(200)
  assert.equal(h.last().progress, .5)
  assert.equal(h.last().elapsedMs, 200)
  h.time(300); h.indicator.accept(event('complete')); h.frame(370)
  assert.equal(h.last().opacity, .5)
})

test('reduced motion still holds for minimum visible time', () => {
  const h = harness({ fadeMs: 0 })
  h.indicator.accept(event('complete')); h.frame(500)
  h.frame(799)
  assert.deepEqual(h.hidden, [])
  assert.equal(h.last().opacity, 1)
  h.frame(800)
  assert.deepEqual(h.hidden, ['a'])
  assert.equal(h.last().opacity, 0)
})

test('teardown cancels rendering and completion callbacks', () => {
  const h = harness()
  h.indicator.accept(event('complete'))
  const oldCallback = [...h.queue.values()][0]
  h.indicator.destroy()
  oldCallback(1000)
  assert.deepEqual(h.hidden, [])
  assert.equal(h.states.length, 0)
})
