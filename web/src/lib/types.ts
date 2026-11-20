// The objects the API lists, as docs/design/api.md and crates/server/src/api.rs give them.
// Times are milliseconds since the Unix epoch.

export interface Stream {
  id: number
  video_id: string
  title: string
  state: 'registered' | 'backfilling' | 'recording' | 'paused' | 'error' | 'ended'
  state_reason: string | null
  period: number | null
  follow_sq: number | null
  last_fetch_at: number | null
  open_fragments: number
  lost_fragments: number
}

export interface Change {
  id: number
  program_id: number
  stream_id: number
  state: 'provisional' | 'confirmed' | 'resolved' | 'dismissed'
  detected_at: number
  dvr_deadline_at: number | null
  start_time: number | null
}

export interface Job {
  id: number
  kind: string
  target: Record<string, unknown> | null
  state: 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled'
  progress: number | null
  error: string | null
  created_at: number
  started_at: number | null
  finished_at: number | null
}

/** Changes that still ask something of the operator. */
export function isOpen(change: Change): boolean {
  return change.state === 'provisional' || change.state === 'confirmed'
}

/** Jobs that can still be cancelled. */
export function isActive(job: Job): boolean {
  return job.state === 'queued' || job.state === 'running'
}
