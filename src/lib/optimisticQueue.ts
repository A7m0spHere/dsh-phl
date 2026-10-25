/**
 * Serialize writes and rebase queued edits on the last successful value.
 *
 * `enqueue` returns a promise for the caller that must not lie about success:
 * it settles when every write queued up to and including this patch has been
 * attempted, resolving `true` only if they all persisted. A failure still
 * fires `onError` (the global toast) and rolls the published value back to the
 * last confirmed state — the promise resolving `false` is how the *originating*
 * UI learns to keep its editing state open instead of congratulating the user.
 */
export function createOptimisticQueue<T extends object>(options: {
  read: () => T
  publish: (patch: Partial<T>) => void
  write: (value: T) => Promise<void>
  onError: (error: unknown) => void
}) {
  let confirmed: T
  const pending: Partial<T>[] = []
  const touched = new Set<keyof T>()
  let running = false
  let completion = Promise.resolve()
  // Waiters joined the currently running drain; each resolves with whether
  // that whole drain persisted everything.
  let waiters: Array<(ok: boolean) => void> = []
  let drainFailed = false

  const publish = () => {
    const next = Object.assign({}, confirmed, ...pending) as T
    const patch: Partial<T> = {}
    for (const key of touched) patch[key] = next[key]
    options.publish(patch)
  }

  const drain = async () => {
    while (pending.length) {
      const next = { ...confirmed, ...pending[0] }
      try {
        await options.write(next)
        confirmed = next
      } catch (error) {
        drainFailed = true
        options.onError(error)
      }
      pending.shift()
      publish()
    }
    touched.clear()
    running = false
    const failed = drainFailed
    drainFailed = false
    const settle = waiters
    waiters = []
    for (const resolve of settle) resolve(!failed)
  }

  return {
    get busy() { return running },
    flush: () => completion,
    enqueue(patch: Partial<T>): Promise<boolean> {
      const start = !running
      if (start) {
        confirmed = options.read()
        running = true
        drainFailed = false
      }
      for (const key of Object.keys(patch) as (keyof T)[]) touched.add(key)
      pending.push(patch)
      publish()
      let resolve!: (ok: boolean) => void
      const done = new Promise<boolean>((r) => (resolve = r))
      waiters.push(resolve)
      if (start) completion = drain()
      return done
    },
  }
}
