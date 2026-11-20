// The page's one connection for live updates, which screens listen to. A screen fetches when
// it mounts and again on each resync, and applies events in between.

import { connect, createHub, type Topic } from './events'

const hub = createHub()
let isConnected = $state(false)
let version = $state<string | null>(null)

connect({
  onEvent: hub.onEvent,
  onResync: hub.onResync,
  onVersion: (running) => {
    version = running
  },
  onConnected: (connected) => {
    isConnected = connected
  },
})

export const live = {
  get isConnected(): boolean {
    return isConnected
  },
  /** The service's version, known once the first resync has come. */
  get version(): string | null {
    return version
  },
}

/** Calls `listener` for each event of a topic, until the returned function is called. */
export function onLive(topic: Topic, listener: (data: unknown) => void): () => void {
  return hub.listen(topic, listener)
}

/**
 * Calls `refetch` whenever what a screen shows may be stale: each time the connection opens
 * (the service sends a resync first) and when the service says the page fell behind. Until the returned function is called.
 */
export function onResync(refetch: () => void): () => void {
  return hub.refetchOn(refetch)
}
