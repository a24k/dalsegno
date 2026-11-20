import { describe, expect, test, vi } from 'vitest'
import { backoff, connect, createHub, EVENTS_URL, type Handlers, type Source } from './events'

class FakeSource implements Source {
  onopen: ((event: Event) => void) | null = null
  onerror: ((event: Event) => void) | null = null
  readonly listeners = new Map<string, (event: MessageEvent) => void>()
  isClosed = false

  addEventListener(type: string, listener: (event: MessageEvent) => void): void {
    this.listeners.set(type, listener)
  }

  close(): void {
    this.isClosed = true
  }

  emit(type: string, data: string): void {
    this.listeners.get(type)?.(new MessageEvent(type, { data }))
  }
}

function setUp() {
  const sources: FakeSource[] = []
  const waits: { run: () => void; ms: number; isCancelled: boolean }[] = []
  const handlers: Handlers = {
    onEvent: vi.fn(),
    onResync: vi.fn(),
    onVersion: vi.fn(),
    onConnected: vi.fn(),
  }
  const disconnect = connect(handlers, {
    open: (url) => {
      expect(url).toBe(EVENTS_URL)
      const source = new FakeSource()
      sources.push(source)
      return source
    },
    wait: (run, ms) => {
      const entry = { run, ms, isCancelled: false }
      waits.push(entry)
      return () => {
        entry.isCancelled = true
      }
    },
    random: () => 1,
  })
  return { sources, waits, handlers, disconnect }
}

describe('backoff', () => {
  test('doubles from one second up to thirty', () => {
    expect([0, 1, 2, 3, 4, 5, 9].map((attempt) => backoff(attempt, () => 1))).toEqual([
      1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000,
    ])
  })

  test('waits at least half the ceiling', () => {
    expect(backoff(2, () => 0)).toBe(2_000)
  })
})

describe('connect', () => {
  test('reports the connection when it opens and leaves the refetch to the resync', () => {
    const { sources, handlers } = setUp()

    sources[0].onopen?.(new Event('open'))

    expect(handlers.onConnected).toHaveBeenCalledWith(true)
    expect(handlers.onResync).not.toHaveBeenCalled()
  })

  test('hands each event over with its data', () => {
    const { sources, handlers } = setUp()

    sources[0].emit('job.progress', '{"id":3,"progress":0.5}')
    sources[0].emit('stream.state', '{"id":1,"state":"following"}')

    expect(vi.mocked(handlers.onEvent).mock.calls).toEqual([
      ['job.progress', { id: 3, progress: 0.5 }],
      ['stream.state', { id: 1, state: 'following' }],
    ])
  })

  test('skips an event whose data does not parse', () => {
    const { sources, handlers } = setUp()

    sources[0].emit('change.updated', 'not json')

    expect(handlers.onEvent).not.toHaveBeenCalled()
  })

  test('refetches when the service says the page fell behind', () => {
    const { sources, handlers } = setUp()

    sources[0].emit('resync', '{}')

    expect(handlers.onResync).toHaveBeenCalledOnce()
  })

  test('reads the version from the resync', () => {
    const { sources, handlers } = setUp()

    sources[0].emit('resync', '{"version":"0.6.0-rc.6"}')

    expect(handlers.onVersion).toHaveBeenCalledWith('0.6.0-rc.6')
    expect(handlers.onResync).toHaveBeenCalledOnce()
  })

  test('ignores a version that is missing or not a string', () => {
    const { sources, handlers } = setUp()

    sources[0].emit('resync', '{}')
    sources[0].emit('resync', '{"version":6}')
    sources[0].emit('resync', 'not json')

    expect(handlers.onVersion).not.toHaveBeenCalled()
    expect(handlers.onResync).toHaveBeenCalledTimes(3)
  })

  test('reconnects with a growing wait, and starts over once connected', () => {
    const { sources, waits, handlers } = setUp()

    sources[0].onerror?.(new Event('error'))
    waits[0].run()
    sources[1].onerror?.(new Event('error'))
    waits[1].run()
    sources[2].onopen?.(new Event('open'))
    sources[2].onerror?.(new Event('error'))

    expect(sources[0].isClosed).toBe(true)
    expect(handlers.onConnected).toHaveBeenCalledWith(false)
    expect(waits.map((entry) => entry.ms)).toEqual([1_000, 2_000, 1_000])
  })

  test('stops reconnecting once disconnected', () => {
    const { sources, waits, disconnect } = setUp()

    sources[0].onerror?.(new Event('error'))
    disconnect()

    expect(waits[0].isCancelled).toBe(true)
    expect(sources).toHaveLength(1)
  })
})

describe('createHub', () => {
  test('hands an event to the listeners of its topic only', () => {
    const hub = createHub()
    const jobs = vi.fn()
    const streams = vi.fn()
    hub.listen('job.progress', jobs)
    hub.listen('stream.state', streams)

    hub.onEvent('job.progress', { id: 1 })

    expect(jobs).toHaveBeenCalledWith({ id: 1 })
    expect(streams).not.toHaveBeenCalled()
  })

  test('stops calling a listener or a refetch once it is removed', () => {
    const hub = createHub()
    const listener = vi.fn()
    const refetch = vi.fn()
    const stopListening = hub.listen('change.updated', listener)
    const stopRefetching = hub.refetchOn(refetch)

    hub.onResync()
    stopListening()
    stopRefetching()
    hub.onEvent('change.updated', { id: 2 })
    hub.onResync()

    expect(listener).not.toHaveBeenCalled()
    expect(refetch).toHaveBeenCalledOnce()
  })
})
