// A page of a list endpoint that live updates keep current (D-33).

import { api, ApiError } from './api'
import type { Topic } from './events'
import { onLive, onResync } from './live.svelte'
import { upsert, type Identified, type Order } from './listing'

export class LiveList<T extends Identified> {
  items = $state<T[]>([])
  total = $state<number | null>(null)
  error = $state<string | null>(null)
  isLoading = $state(true)

  readonly #path: string
  readonly #topic: Topic
  readonly #order: Order
  readonly #limit: number

  constructor(path: string, topic: Topic, order: Order, limit: number) {
    this.#path = path
    this.#topic = topic
    this.#order = order
    this.#limit = limit
  }

  async load(): Promise<void> {
    try {
      const page = await api.get<T[]>(`${this.#path}?limit=${this.#limit}`)
      this.items = page.items
      this.total = page.meta?.total ?? page.items.length
      this.error = null
    } catch (error) {
      this.error = error instanceof ApiError ? error.message : 'The list could not be read.'
    } finally {
      this.isLoading = false
    }
  }

  /**
   * Listens first and then reads, so nothing that changes in between is missed. Returns the
   * function that stops listening, for an `$effect` to hand back.
   */
  start(): () => void {
    const stopListening = onLive(this.#topic, (data) => {
      this.items = upsert(this.items, data as T, this.#order, this.#limit)
    })
    const stopRefetching = onResync(() => void this.load())
    void this.load()
    return () => {
      stopListening()
      stopRefetching()
    }
  }
}

/** How often times shown as "left" or "ago" move on. */
const TICK_MS = 30_000

/** The time now, moving on every half minute while a screen shows it. */
export class Clock {
  now = $state(Date.now())

  /** Starts the clock, and returns the function that stops it. */
  start(): () => void {
    const timer = setInterval(() => {
      this.now = Date.now()
    }, TICK_MS)
    return () => clearInterval(timer)
  }
}
