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
