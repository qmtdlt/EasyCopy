// Browser-only UI acceptance using an explicitly labelled mock Tauri bridge.
// Does not read/write the real clipboard or configure a real relay.
// Run while Vite is serving: node tests/browser-qa.mjs
import { spawn } from 'node:child_process'
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import assert from 'node:assert/strict'

const output = resolve('tests/artifacts')
await mkdir(output, { recursive: true })
const profile = await mkdtemp(`${tmpdir()}/easycopy-browser-qa-`)
const chrome = spawn(process.env.CHROME_PATH || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
  '--headless', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
  '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank',
], { stdio: ['ignore', 'ignore', 'pipe'] })
const calls = new Map()
let serial = 0, socket
const errors = []
function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = ++serial
    calls.set(id, { resolve, reject })
    socket.send(JSON.stringify({ id, method, params }))
  })
}
async function evaluate(expression) {
  const response = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })
  if (response.exceptionDetails) throw new Error(JSON.stringify(response.exceptionDetails))
  return response.result.value
}
async function waitFor(expression) {
  const deadline = Date.now() + 10000
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return
    await new Promise(resolve => setTimeout(resolve, 50))
  }
  throw new Error(`Timed out waiting for ${expression}`)
}
async function screenshot(name) {
  const { data } = await send('Page.captureScreenshot', { format: 'png' })
  await writeFile(resolve(output, name), Buffer.from(data, 'base64'))
}

try {
  const endpoint = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Chrome launch timed out')), 15000)
    chrome.on('error', reject)
    chrome.stderr.on('data', chunk => {
      const match = chunk.toString().match(/DevTools listening on (ws:\/\/[^\s]+)/)
      if (match) { clearTimeout(timeout); resolve(match[1]) }
    })
  })
  const target = (await (await fetch(`http://127.0.0.1:${new URL(endpoint).port}/json/list`)).json()).find(item => item.type === 'page')
  socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject })
  socket.onmessage = event => {
    const response = JSON.parse(event.data)
    if (response.id) {
      const call = calls.get(response.id)
      calls.delete(response.id)
      if (response.error) call?.reject(new Error(JSON.stringify(response.error)))
      else call?.resolve(response.result)
    } else if (response.method === 'Runtime.exceptionThrown') errors.push(response.params)
  }
  await send('Page.enable'); await send('Runtime.enable')
  await send('Emulation.setDeviceMetricsOverride', { width: 1000, height: 700, deviceScaleFactor: 1, mobile: false })
  const thumbnail = `data:image/png;base64,${(await readFile('public/assets/transfer-icons.png')).toString('base64')}`
  const source = `(() => {
    window.isTauri = true;
    const callbacks = new Map(), listeners = new Map(); let n = 0;
    const base = { sequence: 1, createdAt: 1793687520000, thumbnail: null, status: 'complete', available: true, error: null };
    const rows = [
      { ...base, id: 'text-a', kind: 'text', deviceName: 'MacBook Pro', preview: '这台复制，那台粘贴。', detail: '文本、图片和文件，都能在局域网内同步。' },
      { ...base, id: 'image-a', kind: 'image', deviceName: 'Windows', createdAt: base.createdAt - 240000, preview: '图片', detail: '透明 PNG · 界面测试样本', thumbnail: ${JSON.stringify(thumbnail)} },
      { ...base, id: 'files-a', kind: 'files', deviceName: 'Windows', createdAt: base.createdAt - 720000, preview: '接口说明.pdf', detail: 'PDF 文档 · 2.4 MB' },
      { ...base, id: 'text-b', kind: 'text', deviceName: 'MacBook Pro', createdAt: base.createdAt - 960000, preview: '会议时间改为明天下午三点。', detail: '文本' },
      { ...base, id: 'files-b', kind: 'files', deviceName: 'Windows', createdAt: base.createdAt - 1440000, preview: '项目素材', detail: '文件夹 · 18.6 MB' },
    ];
    window.__qaCalls = [];
    window.__qaEmit = (event, payload) => { for (const [id, callback] of listeners.get(event) || []) callbacks.get(callback)?.({ event, id, payload }); };
    window.__TAURI_INTERNALS__ = {
      transformCallback(callback) { const id = ++n; callbacks.set(id, callback); return id; },
      unregisterCallback(id) { callbacks.delete(id); },
      async invoke(cmd, args) {
        window.__qaCalls.push({ cmd, args });
        if (cmd === 'plugin:event|listen') { const id = ++n; const list = listeners.get(args.event) || []; list.push([id,args.handler]); listeners.set(args.event,list); return id; }
        if (cmd === 'plugin:event|unlisten') return;
        if (cmd === 'list_history') return rows;
        if (cmd === 'get_settings') return { address: '192.168.1.10', port: 8080, hasPairingKey: true };
        if (cmd === 'get_connection') return { connected: false, message: '界面测试数据 · 未连接真实服务器' };
        if (cmd === 'test_connection') return { protocol_version: 1, server_id: 'test', logical_name: 'test.invalid' };
        if (cmd === 'copy_item' || cmd === 'save_item' || cmd === 'save_settings' || cmd === 'overlay_ready' || cmd === 'overlay_hidden') return;
        throw new Error('Unmocked command '+cmd);
      }
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    document.addEventListener('DOMContentLoaded', () => {
      if (location.search.includes('overlay')) return;
      const banner = document.createElement('span'); banner.textContent = '界面测试 · 模拟数据';
      banner.style = 'position:fixed;right:20px;top:15px;color:#c8b29b;font:11px system-ui;z-index:20;pointer-events:none'; document.body.append(banner);
    });
  })();`
  await send('Page.addScriptToEvaluateOnNewDocument', { source })
  await send('Page.navigate', { url: process.env.QA_URL || 'http://127.0.0.1:1420/' })
  await waitFor("document.querySelectorAll('.history-row').length === 5")
  await screenshot('history-fixture.png')
  await evaluate("document.querySelectorAll('.history-row')[1].focus()")
  await screenshot('history-hover-fixture.png')
  assert.equal(await evaluate("getComputedStyle(document.querySelectorAll('.item-actions')[1]).opacity"), '1')
  await evaluate("document.querySelectorAll('.history-row')[1].querySelector('.item-actions button').click()")
  await waitFor("window.__qaCalls.some(c => c.cmd === 'copy_item' && c.args.id === 'image-a')")
  await evaluate("document.querySelectorAll('.filters button')[1].click()")
  await waitFor("document.querySelectorAll('.history-row').length === 2")
  await evaluate("document.querySelectorAll('nav button')[1].click()")
  await waitFor("!!document.querySelector('.settings')")
  await screenshot('settings-fixture.png')
  assert.equal(await evaluate("document.querySelector('#pairing-key').type"), 'password')
  await evaluate("document.querySelector('.reveal-key').click()")
  assert.equal(await evaluate("document.querySelector('#pairing-key').type"), 'text')
  await evaluate("document.querySelector('.button.secondary').click()")
  await waitFor("document.querySelector('.form-result').textContent.includes('连接正常')")
  await evaluate("document.querySelector('.button.primary').click()")
  await waitFor("document.querySelector('.form-result').textContent.includes('设置已保存')")
  assert.deepEqual(await evaluate("window.__qaCalls.find(c => c.cmd === 'save_settings').args.input"), { address: '192.168.1.10', port: 8080, pairingKey: '' })
  await send('Page.navigate', { url: `${process.env.QA_URL || 'http://127.0.0.1:1420/'}?overlay=1` })
  await waitFor("window.__qaCalls.some(c => c.cmd === 'overlay_ready')")
  await evaluate("window.__qaEmit('transfer', {taskId:'u',direction:'upload',phase:'start',receivedBytes:0,totalBytes:null,error:null}); window.__qaEmit('transfer', {taskId:'u',direction:'upload',phase:'transferring',receivedBytes:50,totalBytes:100,error:null})")
  await new Promise(resolve => setTimeout(resolve, 100))
  await screenshot('overlay-upload-half.png')
  await evaluate("window.__qaEmit('transfer', {taskId:'d',direction:'download',phase:'start',receivedBytes:0,totalBytes:null,error:null}); window.__qaEmit('transfer', {taskId:'d',direction:'download',phase:'transferring',receivedBytes:50,totalBytes:100,error:null})")
  await new Promise(resolve => setTimeout(resolve, 100))
  await screenshot('overlay-download-half.png')
  await evaluate("window.__qaEmit('transfer', {taskId:'d',direction:'download',phase:'failed',receivedBytes:50,totalBytes:100,error:'校验失败（测试）'})")
  await new Promise(resolve => setTimeout(resolve, 40))
  await screenshot('overlay-failure.png')
  await waitFor("window.__qaCalls.some(c => c.cmd === 'overlay_hidden' && c.args.taskId === 'd')")
  assert.equal(errors.length, 0, JSON.stringify(errors))
  await writeFile(resolve(output, 'browser-qa.json'), JSON.stringify({
    environment: 'Chrome headless, 1000x700, mock Tauri bridge only',
    results: ['history fixture and hover actions', 'copy command arguments', 'type filter', 'settings visibility and save preserves stored key', 'upload/download mirrored rendering', 'failure state and hide callback'],
    consoleExceptions: errors,
  }, null, 2))
  console.log('Browser UI checks passed; screenshots and report: tests/artifacts/')
} finally {
  socket?.close()
  chrome.kill('SIGTERM')
  await new Promise(resolve => chrome.on('exit', resolve))
  await rm(profile, { recursive: true, force: true })
}
