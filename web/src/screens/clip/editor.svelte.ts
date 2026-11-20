// Everything the boundary editor shows: the candidates of a clip, the job that finds them, and
// the edits not yet saved.

import { followDecision } from '../../lib/editor'
import { api, ApiError, errorMessage } from '../../lib/api'
import { onLive, onResync } from '../../lib/live.svelte'
import { type Clip, type ClipCandidates, type Edge, type Job } from '../../lib/types'

type Drafts = Record<Edge, number | null>

const NO_DRAFTS: Drafts = { start: null, end: null }

export class ClipEditor {
  candidates = $state<ClipCandidates | null>(null)
  drafts = $state<Drafts>(NO_DRAFTS)
  isMissing = $state(false)
  error = $state<string | null>(null)
  /** Why the job that was finding the candidates did not finish. */
  jobProblem = $state<string | null>(null)

  /** Set when a job for this clip succeeded and the candidates were read again. */
  hasFinishedJob = false
  /** The job still active after a finished one, which the screen no longer follows. */
  stalledJob = $state<Job | null>(null)

  readonly id: number

  constructor(id: number) {
    this.id = id
  }

  get clip(): Clip | null {
    return this.candidates?.clip ?? null
  }

  /** The job still finding candidates, if any. */
  get finding(): Job | null {
    const job = this.candidates?.job ?? null
    return followDecision(this.hasFinishedJob, job) === 'following' ? job : null
  }

  /** Reads the candidates. A draft whose reports are missing queues the job and answers 202. */
  async load(): Promise<void> {
    try {
      const candidates = (await api.get<ClipCandidates>(`/clips/${this.id}/candidates`)).items
      const decision = followDecision(this.hasFinishedJob, candidates.job)
      this.stalledJob = decision === 'stalled' ? candidates.job : null
      this.hasFinishedJob = decision !== 'ready' && this.hasFinishedJob
      this.candidates = candidates
      this.isMissing = false
      this.error = null
      this.jobProblem = null
    } catch (error) {
      this.isMissing = error instanceof ApiError && error.status === 404
      this.error = errorMessage(error, 'The candidates could not be read.')
    }
  }

  /** Keeps an edit of one edge, or drops it when `time` is null. */
  edit(edge: Edge, time: number | null): void {
    this.drafts = { ...this.drafts, [edge]: time }
  }

  /** The edges that have an edit. */
  get unsaved(): Edge[] {
    return (['start', 'end'] as const).filter((edge) => this.drafts[edge] !== null)
  }

  /** Sends the edits, and reads the candidates again, which may queue the job once more. */
  async save(): Promise<void> {
    const body: Partial<Record<Edge, number>> = {}
    for (const edge of this.unsaved) {
      body[edge] = this.drafts[edge] ?? undefined
    }
    const clip = await api.patch<Clip>(`/clips/${this.id}`, body)
    this.drafts = NO_DRAFTS
    if (this.candidates) {
      this.candidates = { ...this.candidates, clip }
    }
    await this.load()
  }

  /** Reads the candidates once more, as "Try again" does, and follows a new job. */
  async retry(): Promise<void> {
    this.hasFinishedJob = false
    await this.load()
  }

  /** Looks up the job after an event for it: reads again when it succeeded, else says why not. */
  async follow(jobId: number): Promise<void> {
    try {
      const job = (await api.get<Job>(`/jobs/${jobId}`)).items
      if (job.state === 'succeeded') {
        this.hasFinishedJob = true
        await this.load()
      } else if (this.candidates && followDecision(false, job) === 'ready') {
        this.candidates = { ...this.candidates, job: null }
        this.jobProblem = job.error ?? 'The job did not finish.'
      } else if (this.candidates) {
        this.candidates = { ...this.candidates, job }
      }
    } catch {
      await this.load()
    }
  }

  /** Listens first and then reads, as `ChangeDetail.start` does. */
  start(): () => void {
    const stopListening = onLive('job.progress', (data) => {
      const jobId = (data as { id?: number }).id
      if (jobId !== undefined && jobId === this.finding?.id) {
        void this.follow(jobId)
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
