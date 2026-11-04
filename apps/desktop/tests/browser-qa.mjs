// Browser-only UI acceptance using an explicitly labelled mock Tauri bridge.
// Does not read/write the real clipboard or configure a real relay.
// Run while Vite is serving: node tests/browser-qa.mjs
import { spawn } from 'node:child_process'
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import assert from 'node:assert/strict'

const output = resolve(process.env.QA_OUTPUT || 'tests/artifacts')
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
    window.__qaFilePrompt = null;
    window.__qaFileThreshold = 50;
    window.__qaFileThresholdError = "";
    window.__qaAutostart = { enabled: false, canEnable: true, reason: null };
    window.__qaAutostartError = '';
    window.__qaAutostartReadError = '';
    window.__qaMaximized = false;
    window.__qaFullscreen = false;
    window.__qaEmit = (event, payload) => { for (const [id, callback] of listeners.get(event) || []) callbacks.get(callback)?.({ event, id, payload }); };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback(callback) { const id = ++n; callbacks.set(id, callback); return id; },
      unregisterCallback(id) { callbacks.delete(id); },
      async invoke(cmd, args) {
        window.__qaCalls.push({ cmd, args });
        if (cmd === 'plugin:event|listen') { const id = ++n; const list = listeners.get(args.event) || []; list.push([id,args.handler]); listeners.set(args.event,list); return id; }
        if (cmd === 'plugin:event|unlisten') return;
        if (cmd === 'plugin:window|is_fullscreen') return window.__qaFullscreen;
        if (cmd === 'plugin:window|is_maximized') return window.__qaMaximized;
        if (cmd === 'plugin:window|toggle_maximize') { window.__qaMaximized = !window.__qaMaximized; return; }
        if (['plugin:window|minimize', 'plugin:window|close', 'plugin:window|start_dragging'].includes(cmd)) return;
        if (cmd === 'get_file_upload_threshold') return window.__qaFileThreshold;
        if (cmd === 'set_file_upload_threshold') {
          if (window.__qaFileThresholdError) throw new Error(window.__qaFileThresholdError);
          window.__qaFileThreshold = args.mb; return;
        }
        if (cmd === 'get_file_upload_prompt') return window.__qaFilePrompt;
        if (cmd === 'resolve_file_upload') {
          window.__qaFilePrompt = null;
          window.__qaEmit('file-upload-prompt', null);
          return;
        }
        if (cmd === 'list_history') return rows;
        if (cmd === 'get_settings') return { address: '192.168.1.10', port: 8080, hasPairingKey: true };
        if (cmd === 'get_autostart') {
          if (window.__qaAutostartReadError) throw new Error(window.__qaAutostartReadError);
          return {...window.__qaAutostart};
        }
        if (cmd === 'set_autostart') {
          if (window.__qaAutostartError) throw new Error(window.__qaAutostartError);
          window.__qaAutostart.enabled = args.enabled;
          return {...window.__qaAutostart};
        }
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
  await waitFor("window.__qaCalls.some(c => c.cmd === 'plugin:window|is_maximized')")
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.app-shell')).borderTopLeftRadius"), '29px')
  assert.equal(await evaluate("getComputedStyle(document.documentElement).backgroundColor"), 'rgba(0, 0, 0, 0)')
  assert.equal(await evaluate("document.elementFromPoint(0, 0)?.closest('.app-shell') === null"), true)
  await evaluate("document.querySelector('.window-control[aria-label=最大化]').click()")
  await waitFor("document.querySelector('.window-control[aria-label=还原]') !== null")
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.app-shell')).borderTopLeftRadius"), '0px')
  await evaluate("document.querySelector('.window-control[aria-label=还原]').click()")
  await waitFor("document.querySelector('.window-control[aria-label=最大化]') !== null")
  await evaluate("document.querySelector('.window-control[aria-label=最小化]').click()")
  await waitFor("window.__qaCalls.some(c => c.cmd === 'plugin:window|minimize') && !document.querySelector('.window-close').disabled")
  await evaluate("document.querySelector('.window-close').click()")
  await waitFor("window.__qaCalls.some(c => c.cmd === 'plugin:window|close')")
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.app-shell')).borderTopLeftRadius"), '29px')
  await evaluate("window.__qaFullscreen = true; window.__qaEmit('tauri://resize', {width:1000,height:700})")
  await waitFor("getComputedStyle(document.querySelector('.app-shell')).borderTopLeftRadius === '0px'")
  await evaluate("window.__qaFullscreen = false; window.__qaEmit('tauri://resize', {width:1000,height:700})")
  await waitFor("getComputedStyle(document.querySelector('.app-shell')).borderTopLeftRadius === '29px'")
  await screenshot('history-fixture.png')
  await evaluate("document.querySelectorAll('.history-row')[1].focus()")
  await screenshot('history-hover-fixture.png')
  assert.equal(await evaluate("getComputedStyle(document.querySelectorAll('.item-actions')[1]).opacity"), '1')
  await evaluate("document.querySelectorAll('.history-row')[1].querySelector('.item-actions button').click()")
  await waitFor("window.__qaCalls.some(c => c.cmd === 'copy_item' && c.args.id === 'image-a')")
  assert.deepEqual(await evaluate("Array.from(document.querySelectorAll('.history-row')[2].querySelectorAll('.item-actions button'), b => b.textContent.trim())"), ['另存为'])
  await evaluate("document.querySelectorAll('.history-row')[2].querySelector('.item-actions button').click()")
  await waitFor("window.__qaCalls.some(c => c.cmd === 'save_item' && c.args.id === 'files-a')")
  assert.equal(await evaluate("window.__qaCalls.some(c => c.cmd === 'copy_item' && c.args.id.startsWith('files'))"), false)
  for (const [id, action, expected] of [['large-default', 'enter', false], ['large-escape', 'escape', false], ['large-close', 'close', false], ['large-yes', 'yes', true]]) {
    await evaluate(`window.__qaFilePrompt = { id: '${id}', bytes: 75000001, thresholdMb: 50 }; window.__qaEmit('file-upload-prompt', window.__qaFilePrompt)`)
    await waitFor("document.querySelector('.file-upload-dialog').open && document.activeElement.textContent.trim() === '否'")
    if (action === 'enter') {
      await screenshot('file-upload-confirmation.png')
      await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 })
      await send('Input.dispatchKeyEvent', { type: 'char', text: '\r', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 })
      await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 })
    } else if (action === 'escape') {
      await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
      await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
    } else await evaluate(`document.querySelector('${action === 'close' ? '.file-upload-close' : '.file-upload-actions .secondary'}').click()`)
    try { await waitFor("!document.querySelector('.file-upload-dialog').open") }
    catch (error) { throw new Error(`${id}: ${error.message}; state=${JSON.stringify(await evaluate("({focus:document.activeElement.textContent, calls:window.__qaCalls.slice(-4)})"))}`) }
    assert.deepEqual(await evaluate(`window.__qaCalls.find(c => c.cmd === 'resolve_file_upload' && c.args.id === '${id}').args`), { id, upload: expected })
  }
  await evaluate("window.__qaEmit('file-upload-prompt', { id: 'superseded', bytes: 80000000, thresholdMb: 50 })")
  await waitFor("document.querySelector('.file-upload-dialog').open")
  await evaluate("window.__qaEmit('file-upload-prompt', null)")
  await waitFor("!document.querySelector('.file-upload-dialog').open")
  assert.equal(await evaluate("window.__qaCalls.some(c => c.cmd === 'resolve_file_upload' && c.args.id === 'superseded')"), false)
  await evaluate("document.querySelectorAll('.filters button')[1].click()")
  await waitFor("document.querySelectorAll('.history-row').length === 2")
  await evaluate("document.querySelectorAll('nav button')[1].click()")
  await waitFor("!!document.querySelector('.settings')")
  await waitFor("document.querySelector('[role=switch]').disabled === false")
  await waitFor("document.querySelector('#file-upload-threshold').disabled === false")
  assert.equal(await evaluate("document.querySelector('#file-upload-threshold').value"), '50')
  assert.equal(await evaluate("window.__qaCalls.some(c => c.cmd === 'set_file_upload_threshold')"), false)
  for (const value of [10, 100, 37]) {
    await evaluate(`{ const field = document.querySelector('#file-upload-threshold'); field.value = '${value}'; field.dispatchEvent(new Event('input', { bubbles: true })); document.querySelector('.file-upload-setting button').click(); }`)
    await waitFor(`window.__qaFileThreshold === ${value} && !document.querySelector('#file-upload-threshold').disabled`)
  }
  await evaluate("window.__qaFileThresholdError = '保存失败（测试）'; document.querySelector('.file-upload-setting button').click()")
  await waitFor("document.querySelector('.file-upload-setting-result').textContent.includes('保存失败')")
  await screenshot('settings-file-threshold-error.png')
  await evaluate("window.__qaFileThresholdError = ''; { const field = document.querySelector('#file-upload-threshold'); field.value = '50'; field.dispatchEvent(new Event('input', { bubbles: true })); document.querySelector('.file-upload-setting button').click(); }")
  await waitFor("window.__qaFileThreshold === 50 && document.querySelector('.file-upload-setting-result').textContent.includes('已保存')")
  await screenshot('settings-fixture.png')
  assert.equal(await evaluate("document.querySelector('[role=switch]').getAttribute('aria-checked')"), 'false')
  assert.equal(await evaluate("window.__qaCalls.some(c => c.cmd === 'set_autostart')"), false, 'reading settings must not register startup')
  await evaluate("document.querySelector('[role=switch]').click()")
  await waitFor("document.querySelector('[role=switch]').getAttribute('aria-checked') === 'true'")
  await evaluate("document.querySelector('[role=switch]').click()")
  await waitFor("document.querySelector('[role=switch]').getAttribute('aria-checked') === 'false'")
  assert.deepEqual(await evaluate("window.__qaCalls.filter(c => c.cmd === 'set_autostart').map(c => c.args)"), [{enabled:true}, {enabled:false}])
  await evaluate("window.__qaAutostartError = '注册失败（测试）'; document.querySelector('[role=switch]').click()")
  await waitFor("document.querySelector('#autostart-result').textContent.includes('注册失败')")
  assert.equal(await evaluate("document.querySelector('[role=switch]').getAttribute('aria-checked')"), 'false')
  await evaluate("window.__qaAutostartError = ''; window.__qaAutostart.enabled = true; window.dispatchEvent(new Event('focus'))")
  await waitFor("document.querySelector('[role=switch]').getAttribute('aria-checked') === 'true'")
  await evaluate("window.__qaAutostartReadError = '读取失败（测试）'; window.dispatchEvent(new Event('focus'))")
  await waitFor("document.querySelector('#autostart-result').textContent.includes('读取失败')")
  assert.equal(await evaluate("document.querySelector('[role=switch]').disabled"), true)
  await evaluate("window.__qaAutostartReadError = ''; document.querySelector('.autostart-retry').click()")
  await waitFor("document.querySelector('[role=switch]').getAttribute('aria-checked') === 'true'")
  await evaluate("window.__qaAutostart = {enabled:false, canEnable:false, reason:'开发调试版本不支持开启自启动'}; window.dispatchEvent(new Event('focus'))")
  await waitFor("document.querySelector('#autostart-result').textContent.includes('开发调试版本')")
  assert.equal(await evaluate("document.querySelector('[role=switch]').disabled"), true)
  await screenshot('settings-autostart-development.png')
  // The added row and its error/retry text must remain reachable at the minimum window size.
  await send('Emulation.setDeviceMetricsOverride', { width: 760, height: 540, deviceScaleFactor: 1, mobile: false })
  assert.equal(await evaluate("document.querySelector('.settings').scrollWidth <= document.querySelector('.settings').clientWidth"), true)
  await screenshot('settings-autostart-minimum.png')
  await evaluate("document.querySelector('#file-upload-threshold').scrollIntoView({block:'center'})")
  assert.equal(await evaluate("document.querySelector('#file-upload-threshold').getBoundingClientRect().bottom < innerHeight"), true)
  await screenshot('settings-file-threshold-minimum.png')
  await send('Emulation.setDeviceMetricsOverride', { width: 1000, height: 700, deviceScaleFactor: 1, mobile: false })
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
    results: ['window minimize, maximize/restore and close commands', 'file history save-only, default decline, Escape, close, explicit approval and stale prompt dismissal', 'file threshold default, custom values, persistence command and save failure', 'history fixture and hover actions', 'copy command arguments', 'type filter', 'settings visibility and save preserves stored key', 'autostart defaults off without writes, enable/disable, failed write readback, external state refresh, read failure/retry, development restriction, minimum width', 'upload/download mirrored rendering', 'failure state and hide callback'],
    consoleExceptions: errors,
  }, null, 2))
  console.log(`Browser UI checks passed; screenshots and report: ${output}`)
} finally {
  socket?.close()
  chrome.kill('SIGTERM')
  await new Promise(resolve => chrome.on('exit', resolve))
  await rm(profile, { recursive: true, force: true })
}
