// Everything the change screen shows, read from the API and kept current by live updates.

import { api, ApiError, errorMessage } from '../../lib/api'
import { onLive, onResync } from '../../lib/live.svelte'
import type { ChangeDetail as ChangeBody, Comparison, Stream } from '../../lib/types'

export class ChangeDetail {
  change = $state<ChangeBody | null>(null)
  comparison = $state<Comparison | null>(null)
  streamTitle = $state<string | null>(null)
  isMissing = $state(false)
  error = $state<string | null>(null)
  isLoading = $state(true)

  readonly id: number

  constructor(id: number) {
    this.id = id
  }

  async load(): Promise<void> {
    try {
      const [change, comparison] = await Promise.all([
        api.get<ChangeBody>(`/changes/${this.id}`),
        api.get<Comparison>(`/changes/${this.id}/diff`),
      ])
      this.change = change.items
      this.comparison = comparison.items
      await this.loadStreamTitle(change.items.stream_id)
      this.isMissing = false
      this.error = null
    } catch (error) {
      this.isMissing = error instanceof ApiError && error.status === 404
      this.error = errorMessage(error, 'The change could not be read.')
    } finally {
      this.isLoading = false
    }
  }

  /** Reads the stream's title; the screen falls back to its number when that fails. */
  async loadStreamTitle(streamId: number): Promise<void> {
    try {
      this.streamTitle = (await api.get<Stream>(`/streams/${streamId}`)).items.title
    } catch {
      this.streamTitle = null
    }
  }

  /** Takes the change the service answered a dismissal with, and reads the bars again. */
  async show(change: ChangeBody): Promise<void> {
    this.change = change
    await this.load()
  }

  /** Listens first and then reads, as `LiveList.start` does. */
  start(): () => void {
    const stopListening = onLive('change.updated', (data) => {
      if ((data as { id?: number }).id === this.id) {
        void this.load()
      }
    })
    const stopRefetching = onResync(() => void this.load())
    void this.load()
    return () => {
      stopListening()
      stopRefetching()
    }
  }
}
