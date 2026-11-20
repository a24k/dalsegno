// Everything the stream screen shows, read from the API and kept current by live updates.

import { api, ApiError, errorMessage } from '../../lib/api'
import { onLive, onResync } from '../../lib/live.svelte'
import type { Change, KnownClip, Program, Stream, Version } from '../../lib/types'

/** Programs, versions and changes are read in one page each; a program has few. */
const PAGE = 200

export class StreamDetail {
  stream = $state<Stream | null>(null)
  program = $state<Program | null>(null)
  programs = $state<Program[]>([])
  versions = $state<Version[]>([])
  changes = $state<Change[]>([])
  clips = $state<KnownClip[]>([])
  isMissing = $state(false)
  error = $state<string | null>(null)
  isLoading = $state(true)

  readonly id: number

  constructor(id: number) {
    this.id = id
  }

  /** Takes a stream the service just answered with, and reads the program it points to. */
  async show(stream: Stream): Promise<void> {
    this.stream = stream
    await this.refreshProgram()
  }

  /** Reads the program again, and says so on the screen when that fails. */
  async refreshProgram(): Promise<void> {
    try {
      await this.loadProgram()
      this.error = null
    } catch (error) {
      this.error = errorMessage(error, 'The program could not be read.')
    }
  }

  async load(): Promise<void> {
    try {
      const page = await api.get<Stream>(`/streams/${this.id}`)
      this.stream = page.items
      this.programs = (await api.get<Program[]>(`/programs?limit=${PAGE}`)).items
      await this.loadProgram()
      this.error = null
    } catch (error) {
      this.isMissing = error instanceof ApiError && error.status === 404
      this.error = errorMessage(error, 'The stream could not be read.')
    } finally {
      this.isLoading = false
    }
  }

  /** Reads the linked program with its versions, changes and known material. */
  async loadProgram(): Promise<void> {
    const programId = this.stream?.program_id ?? null
    if (programId === null) {
      this.program = null
      this.versions = []
      this.changes = []
      this.clips = []
      return
    }
    const base = `/programs/${programId}`
    const [program, versions, changes, clips] = await Promise.all([
      api.get<Program>(base),
      api.get<Version[]>(`${base}/versions?limit=${PAGE}`),
      api.get<Change[]>(`${base}/changes?limit=${PAGE}`),
      api.get<KnownClip[]>(`${base}/known-clips`),
    ])
    this.program = program.items
    this.versions = versions.items
    this.changes = changes.items
    this.clips = clips.items
  }

  /** Re-reads the programs, as a new one may have been made. */
  async loadPrograms(): Promise<void> {
    this.programs = (await api.get<Program[]>(`/programs?limit=${PAGE}`)).items
  }

  /** Listens first and then reads, as `LiveList.start` does. */
  start(): () => void {
    const stopListening = onLive('stream.state', (data) => {
      const update = data as Stream
      if (update.id === this.id && this.stream) {
        const isRelinked = 'program_id' in update && update.program_id !== this.stream.program_id
        this.stream = { ...this.stream, ...update }
        // A link made elsewhere, such as from the CLI, brings another program.
        if (isRelinked) {
          void this.refreshProgram()
        }
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
