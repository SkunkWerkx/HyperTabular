// Loads one URL in headless Chrome and prints the page's DOM once its title is PASS or FAIL, or
// once the deadline passes, then exits 0 for PASS and 1 otherwise. check.sh runs it per leg.
//
//   node browser.mjs <chrome> <url> [deadline-seconds]
//
// Why not `chrome --dump-dom --virtual-time-budget`: virtual time runs out the moment the
// network goes idle, while the runtime is still compiling dotnet.native.wasm off the main
// thread, so a large module (a Debug build's, unstripped) was dumped at "97%" with nothing run.
// This waits in real time instead, over the DevTools protocol (Node 22's WebSocket, no
// dependency), and prints every console error and uncaught exception on the way, so a failure
// says why.
import { spawn } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'

const [chrome, url, deadlineArg = '120'] = process.argv.slice(2)
if (!chrome || !url) {
  console.error('usage: node browser.mjs <chrome> <url> [deadline-seconds]')
  process.exit(2)
}
const deadline = Date.now() + Number(deadlineArg) * 1000
const profile = mkdtempSync(join(tmpdir(), 'wasm-smoke-'))
const browser = spawn(
  chrome,
  [
    '--headless=new',
    '--no-sandbox',
    '--disable-gpu',
    '--no-first-run',
    '--remote-debugging-port=0',
    `--user-data-dir=${profile}`,
    'about:blank',
  ],
  { stdio: 'ignore' },
)

let status = 1
try {
  // Chrome writes the port it chose to DevToolsActivePort in the profile.
  let port
  while (!port) {
    if (Date.now() > deadline) throw new Error('Chrome never opened its DevTools port')
    try {
      port = readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]
    } catch {
      await sleep(100)
    }
  }
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()
  const page = targets.find((t) => t.type === 'page')
  const socket = new WebSocket(page.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => {
    socket.onopen = resolve
    socket.onerror = reject
  })

  let next = 0
  const pending = new Map()
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data)
    if (message.id !== undefined) {
      pending.get(message.id)?.(message)
      pending.delete(message.id)
    } else if (message.method === 'Runtime.exceptionThrown') {
      const details = message.params.exceptionDetails
      console.error(`   uncaught: ${details.exception?.description ?? details.text}`)
    } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
      console.error(
        `   console.error: ${message.params.args.map((a) => a.value ?? a.description).join(' ')}`,
      )
    } else if (message.method === 'Log.entryAdded' && message.params.entry.level === 'error') {
      console.error(
        `   ${message.params.entry.source}: ${message.params.entry.text} ${message.params.entry.url ?? ''}`,
      )
    }
  }
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      const id = next++
      pending.set(id, resolve)
      socket.send(JSON.stringify({ id, method, params }))
    })
  const evaluate = async (expression) =>
    (await send('Runtime.evaluate', { expression, returnByValue: true })).result?.result?.value

  await send('Runtime.enable')
  await send('Log.enable')
  await send('Page.navigate', { url })

  let title = ''
  while (Date.now() < deadline) {
    title = (await evaluate('document.title')) ?? ''
    if (title === 'PASS' || title === 'FAIL') break
    await sleep(250)
  }
  process.stdout.write(`${(await evaluate('document.documentElement.outerHTML')) ?? ''}\n`)
  status = title === 'PASS' ? 0 : 1
  socket.close()
} catch (error) {
  console.error(`   browser: ${error.message}`)
} finally {
  browser.kill()
  await new Promise((resolve) =>
    browser.exitCode !== null ? resolve() : browser.once('exit', resolve),
  )
  rmSync(profile, { recursive: true, force: true })
}
process.exit(status)
