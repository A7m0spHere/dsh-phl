import { describe, expect, it, vi } from 'vitest'
import { createOptimisticQueue } from './optimisticQueue'

function deferred() {
  let resolve!: () => void
  let reject!: (error: Error) => void
  const promise = new Promise<void>((ok, fail) => { resolve = ok; reject = fail })
  return { promise, resolve, reject }
}

function fixture() {
  let value = { name: 'original', favorite: false, diskUsage: 0 }
  const writes: ReturnType<typeof deferred>[] = []
  const write = vi.fn(() => {
    const operation = deferred()
    writes.push(operation)
    return operation.promise
  })
  const onError = vi.fn()
  const queue = createOptimisticQueue({
    read: () => value,
    publish: (patch) => { value = { ...value, ...patch } },
    write, onError,
  })
  return { queue, write, writes, onError, read: () => value, measure: () => { value.diskUsage = 42 } }
}

describe('optimistic persistence', () => {
  it('serializes writes while publishing edits immediately', async () => {
    const f = fixture()
    f.queue.enqueue({ name: 'first' })
    f.queue.enqueue({ name: 'second' })
    expect(f.read().name).toBe('second')
    expect(f.write).toHaveBeenCalledTimes(1)
    f.writes[0].resolve()
    await Promise.resolve()
    expect(f.write).toHaveBeenLastCalledWith({ name: 'second', favorite: false, diskUsage: 0 })
    f.writes[1].resolve()
    await f.queue.flush()
    expect(f.queue.busy).toBe(false)
  })

  it('rebases a different-field edit after an earlier failure', async () => {
    const f = fixture()
    f.queue.enqueue({ name: 'failed' })
    f.queue.enqueue({ favorite: true })
    f.writes[0].reject(new Error('disk full'))
    await Promise.resolve()
    expect(f.write).toHaveBeenLastCalledWith({ name: 'original', favorite: true, diskUsage: 0 })
    f.writes[1].resolve()
    await f.queue.flush()
    expect(f.read()).toEqual({ name: 'original', favorite: true, diskUsage: 0 })
    expect(f.onError).toHaveBeenCalledTimes(1)
  })

  it('restores the last saved value when consecutive same-field edits fail', async () => {
    const f = fixture()
    f.queue.enqueue({ name: 'first' })
    f.queue.enqueue({ name: 'second' })
    f.writes[0].reject(new Error('first failure'))
    await Promise.resolve()
    expect(f.read().name).toBe('second')
    f.writes[1].reject(new Error('second failure'))
    await f.queue.flush()
    expect(f.read().name).toBe('original')
  })

  it('keeps independently refreshed derived data during a rollback', async () => {
    const f = fixture()
    f.queue.enqueue({ name: 'failed' })
    f.measure()
    f.writes[0].reject(new Error('failure'))
    await f.queue.flush()
    expect(f.read()).toEqual({ name: 'original', favorite: false, diskUsage: 42 })
    f.queue.enqueue({ favorite: true })
    expect(f.write).toHaveBeenLastCalledWith({ name: 'original', favorite: true, diskUsage: 42 })
    f.writes[1].resolve()
    await f.queue.flush()
  })
})

describe('enqueue outcome reporting (#5: no success toast before persistence)', () => {
  it('resolves true when every write in the batch persisted', async () => {
    const f = fixture()
    const first = f.queue.enqueue({ name: 'first' })
    const second = f.queue.enqueue({ favorite: true })
    f.writes[0].resolve()
    await Promise.resolve() // let the drain advance to the second write
    f.writes[1].resolve()
    expect(await first).toBe(true)
    expect(await second).toBe(true)
  })

  it('resolves false when the write failed, alongside onError', async () => {
    const f = fixture()
    const done = f.queue.enqueue({ name: 'doomed' })
    f.writes[0].reject(new Error('disk full'))
    expect(await done).toBe(false)
    expect(f.onError).toHaveBeenCalledTimes(1)
    // The published value rolled back to the last confirmed state.
    expect(f.read().name).toBe('original')
  })

  it('reports the failure to every waiter of the same batch', async () => {
    const f = fixture()
    const a = f.queue.enqueue({ name: 'a' })
    const b = f.queue.enqueue({ favorite: true })
    f.writes[0].reject(new Error('nope'))
    await Promise.resolve() // let the drain advance to the second write
    f.writes[1].resolve()
    expect(await a).toBe(false)
    expect(await b).toBe(false)
  })

  it('does not poison the next batch with an earlier failure', async () => {
    const f = fixture()
    const failed = f.queue.enqueue({ name: 'failed' })
    f.writes[0].reject(new Error('transient'))
    await failed
    const retried = f.queue.enqueue({ name: 'retried' })
    f.writes[1].resolve()
    expect(await retried).toBe(true)
    expect(f.read().name).toBe('retried')
  })

  it('a failed first write keeps the queued second edit honest', async () => {
    // Rapid consecutive edits: the second edit rides the same batch, and a
    // failure of the first write must make BOTH waiters report failure — the
    // UI that closed its form on the second enqueue would otherwise claim a
    // success whose value was never persisted.
    const f = fixture()
    const first = f.queue.enqueue({ name: 'first' })
    f.writes[0].reject(new Error('locked'))
    const second = f.queue.enqueue({ name: 'second' })
    await Promise.resolve() // let the drain advance to the second write
    f.writes[1].resolve()
    expect(await first).toBe(false)
    expect(await second).toBe(false)
    expect(f.read().name).toBe('second')
  })
})
