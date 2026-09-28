/**
 * Shared raw-CDP plumbing for the e2e GUI lanes (`trial-copy-cdp.mjs`,
 * `dependency-retry-cdp.mjs`).
 *
 * No Playwright, no WinAppDriver: WebView2's DevTools port speaks plain CDP,
 * so Node's own `fetch` + `WebSocket` drive a real window. Three things every
 * lane needs live here:
 *
 *  - `connectCdp` / `cdpClient` — the tiny client (evaluate + events);
 *  - page helpers that wait on RENDERED TEXT / element state, never on sleeps
 *    alone (a lane that sleeps is a lane that flakes);
 *  - `ipcRecorder` — Tauri v2 carries commands as
 *    `POST http://ipc.localhost/<command>` with the argument object as the
 *    body, so CDP's Network domain yields the REAL request the page sent.
 *    (Patching `window.__TAURI_INTERNALS__.invoke` is not an option: its
 *    members are non-writable and non-configurable, and this build's IPC does
 *    not travel over `chrome.webview.postMessage`.)
 */
import process from 'node:process'

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

export const json = (v) => JSON.stringify(v)

export async function waitForHttp(url, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url)
      if (res.ok) return
    } catch {
      /* not up yet */
    }
    await sleep(300)
  }
  throw new Error(`${label}: ${url} never answered within ${timeoutMs} ms`)
}

export async function connectCdp(port, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs
  let lastError = null
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/list`)
      if (res.ok) {
        const page = (await res.json()).find((t) => t.type === 'page' && t.url.startsWith('http'))
        if (page?.webSocketDebuggerUrl) return page
      }
    } catch (err) {
      lastError = err
    }
    await sleep(400)
  }
  throw new Error(
    `no page target on DevTools port ${port} within ${timeoutMs} ms` +
      (lastError ? ` (last error: ${lastError})` : ''),
  )
}

export function cdpClient(wsUrl) {
  const ws = new WebSocket(wsUrl)
  let id = 0
  const pending = new Map()
  const listeners = []
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data)
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg)
      pending.delete(msg.id)
    } else if (msg.method) {
      for (const fn of listeners) fn(msg)
    }
  }
  const open = new Promise((resolve, reject) => {
    ws.onopen = resolve
    ws.onerror = () => reject(new Error('cdp websocket failed'))
  })
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      const n = ++id
      pending.set(n, resolve)
      ws.send(JSON.stringify({ id: n, method, params }))
    })
  return {
    ready: open,
    send,
    onEvent: (fn) => listeners.push(fn),
    close: () => ws.close(),
    async evaluate(expression, { awaitPromise = false } = {}) {
      const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise })
      const ex = r.result?.exceptionDetails
      if (ex) throw new Error(`page eval threw: ${JSON.stringify(ex).slice(0, 400)}`)
      return r.result?.result?.value
    },
    async bodyText() {
      return (await this.evaluate('document.body.innerText')) ?? ''
    },
  }
}

/** Waits until every needle appears in the rendered text. */
export async function waitForText(cdp, needles, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs
  let text = ''
  while (Date.now() < deadline) {
    text = await cdp.bodyText()
    if (needles.every((n) => text.includes(n))) return text
    await sleep(300)
  }
  throw new Error(`${label}: missing ${json(needles)}. Page text:\n${text.slice(0, 1500)}`)
}

/** Waits until every needle appears and no forbidden needle does. */
export async function waitForTextWithout(cdp, needles, forbidden, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs
  let text = ''
  while (Date.now() < deadline) {
    text = await cdp.bodyText()
    if (needles.every((n) => text.includes(n)) && !forbidden.some((n) => text.includes(n))) {
      return text
    }
    await sleep(300)
  }
  throw new Error(
    `${label}: expected ${json(needles)} without ${json(forbidden)}. Page text:\n${text.slice(0, 1500)}`,
  )
}

/** Clicks the first button/link/menuitem whose text or aria-label matches. */
export async function clickText(cdp, needle, { exact = false } = {}) {
  const expr = `(() => {
    const nodes = [...document.querySelectorAll('button, a, [role="menuitem"], [role="button"]')]
    const hit = nodes.find((n) => {
      const t = (n.textContent || '').trim()
      const label = (n.getAttribute('aria-label') || '').trim()
      const match = (v) => ${exact ? 'v === ' : 'v.includes('}${json(needle)})
      return match(t) || match(label)
    })
    if (!hit) return { ok: false, seen: nodes.map((n) => (n.textContent || '').trim() || (n.getAttribute('aria-label') || '').trim()).filter(Boolean).slice(0, 40) }
    hit.click()
    return { ok: true, text: (hit.textContent || '').trim() || (hit.getAttribute('aria-label') || '').trim(), disabled: !!hit.disabled }
  })()`
  const result = await cdp.evaluate(expr)
  if (!result?.ok) {
    throw new Error(`click ${json(needle)} failed; candidates: ${JSON.stringify(result?.seen)}`)
  }
  return result
}

/** Waits until the button with this text exists AND is enabled. */
export async function waitForEnabledButton(cdp, needle, timeoutMs, label) {
  const expr = `(() => {
    const hit = [...document.querySelectorAll('button')].find((n) => (n.textContent || '').trim().includes(${json(needle)}))
    return hit ? { found: true, disabled: !!hit.disabled } : { found: false }
  })()`
  const deadline = Date.now() + timeoutMs
  let last = null
  while (Date.now() < deadline) {
    last = await cdp.evaluate(expr)
    if (last?.found && !last.disabled) return
    await sleep(300)
  }
  throw new Error(`${label}: button ${json(needle)} never became enabled (last=${JSON.stringify(last)})`)
}

/** Records every command the page sends, verbatim, from the IPC transport. */
export function ipcRecorder(cdp) {
  const calls = []
  cdp.onEvent(async (msg) => {
    if (msg.method !== 'Network.requestWillBeSent') return
    const { url, method, postData } = msg.params.request
    if (method !== 'POST' || !url.startsWith('http://ipc.localhost/')) return
    let body = postData
    if (body === undefined) {
      const got = await cdp.send('Network.getRequestPostData', { requestId: msg.params.requestId })
      body = got?.result?.postData
    }
    calls.push({
      command: decodeURIComponent(url.slice('http://ipc.localhost/'.length)),
      at: new Date().toISOString(),
      body: body ?? null,
    })
  })
  return calls
}

/** The evidence writer both lanes share: JSON next to the run's screenshots. */
export function writeEvidence(outDir, fileName, evidence, fs, path) {
  fs.mkdirSync(outDir, { recursive: true })
  const target = path.join(outDir, fileName)
  fs.writeFileSync(target, JSON.stringify(evidence, null, 2))
  return target
}

/** Verbose node deprecation notice suppression for `shell: true` spawns. */
export const quietWarnings = () => {
  process.removeAllListeners('warning')
}
