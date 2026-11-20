// Live updates from GET /api/v1/events, with reconnection.
//
// Events carry no history, so whenever the connection opens, and whenever the service says
// this page fell behind, the page refetches what it shows (onResync). A page subscribes
// first and fetches on that call, so it misses nothing in between.

import { API_BASE } from './api'

export const EVENTS_URL = `${API_BASE}/events`

export const TOPICS = ['job.progress', 'stream.state', 'change.updated'] as const
export type Topic = (typeof TOPICS)[number]

const RESYNC = 'resync'
/** The first wait before reconnecting, doubled after each failure up to the longest. */
const FIRST_DELAY_MS = 1_000
const LONGEST_DELAY_MS = 30_000

export interface Handlers {
  onEvent(topic: Topic, data: unknown): void
  onResync(): void
  onConnected(isConnected: boolean): void
}

/** The part of EventSource this module uses, so tests can stand in for it. */
export interface Source {
  onopen: ((event: Event) => void) | null
  onerror: ((event: Event) => void) | null
  addEventListener(type: string, listener: (event: MessageEvent) => void): void
  close(): void
}

export interface Options {
  open?: (url: string) => Source
  wait?: (run: () => void, ms: number) => () => void
  random?: () => number
}

/** How long to wait before the given reconnection attempt, counted from zero. */
export function backoff(attempt: number, random: () => number = Math.random): number {
  const ceiling = Math.min(LONGEST_DELAY_MS, FIRST_DELAY_MS * 2 ** attempt)
  // Jitter keeps several tabs from reconnecting at the same moment.
  return Math.round(ceiling * (0.5 + random() / 2))
}

function parse(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

/** Connects and keeps reconnecting until the returned function is called. */
export function connect(handlers: Handlers, options: Options = {}): () => void {
  const open = options.open ?? ((url) => new EventSource(url))
  const wait =
    options.wait ??
    ((run, ms) => {
      const timer = setTimeout(run, ms)
      return () => clearTimeout(timer)
    })
  const random = options.random ?? Math.random

  let attempt = 0
  let source: Source | null = null
  let cancelWait: (() => void) | null = null
  let isStopped = false

  function start(): void {
    const current = open(EVENTS_URL)
    source = current
    current.onopen = () => {
      attempt = 0
      handlers.onConnected(true)
      handlers.onResync()
    }
    current.onerror = () => {
      // EventSource retries on its own at a fixed pace; this backs off instead.
      current.close()
      handlers.onConnected(false)
      if (!isStopped) {
        cancelWait = wait(start, backoff(attempt, random))
        attempt += 1
      }
    }
    current.addEventListener(RESYNC, () => handlers.onResync())
    for (const topic of TOPICS) {
      current.addEventListener(topic, (event) => {
        const data = parse(event.data)
        if (data !== null) {
          handlers.onEvent(topic, data)
        }
      })
    }
  }

  start()
  return () => {
    isStopped = true
    cancelWait?.()
    source?.close()
  }
}

type Listener = (data: unknown) => void

/** Hands events and resyncs on to the screens that listen for them. */
export function createHub() {
  const listeners = new Map<Topic, Set<Listener>>()
  const refetches = new Set<() => void>()
  return {
    onEvent(topic: Topic, data: unknown): void {
      listeners.get(topic)?.forEach((listener) => listener(data))
    },
    onResync(): void {
      refetches.forEach((refetch) => refetch())
    },
    /** Calls `listener` for each event of a topic, until the returned function is called. */
    listen(topic: Topic, listener: Listener): () => void {
      const set = listeners.get(topic) ?? new Set<Listener>()
      listeners.set(topic, set)
      set.add(listener)
      return () => set.delete(listener)
    },
    /** Calls `refetch` on each resync, until the returned function is called. */
    refetchOn(refetch: () => void): () => void {
      refetches.add(refetch)
      return () => refetches.delete(refetch)
    },
  }
}
