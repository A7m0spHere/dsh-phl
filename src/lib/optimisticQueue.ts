/** Serialize writes and rebase queued edits on the last successful value. */
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
        options.onError(error)
      }
      pending.shift()
      publish()
    }
    touched.clear()
    running = false
  }

  return {
    get busy() { return running },
    flush: () => completion,
    enqueue(patch: Partial<T>) {
      const start = !running
      if (start) {
        confirmed = options.read()
        running = true
      }
      for (const key of Object.keys(patch) as (keyof T)[]) touched.add(key)
      pending.push(patch)
      publish()
      if (start) completion = drain()
    },
  }
}
