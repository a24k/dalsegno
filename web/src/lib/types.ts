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
  program_id: number | null
}

/** A program, the thing a stream loops through. */
export interface Program {
  id: number
  short_name: string
  channel_id: number | null
  name_template: string
  addition_suffix: string
  artist: string | null
  content_date: string | null
  description: string | null
  output_dir: string
  stream_ids: number[]
}

/** One version of a program seen on a stream. Times are stream seconds, except `first_seen_at`. */
export interface Version {
  id: number
  stream_id: number
  period_audio: number
  period_video: number | null
  anchor_time: number
  first_seen_at: number
  on_air_from: number
  on_air_until: number | null
  last_cycle_start: number | null
}

/** Material of a program that is known and stored; analysis masks it from M7. */
export interface KnownClip {
  id: number
  name: string
  duration: number
  feature: string
}

/** What `POST /streams` answers: the new stream and what the probe found. */
export interface AddedStream {
  stream: Stream
  probe: Record<string, unknown>
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

/** Streams that can be started, as the service allows it. */
export function canStart(stream: Stream): boolean {
  return stream.state === 'registered' || stream.state === 'paused' || stream.state === 'error'
}

/** Streams that can be stopped. */
export function canStop(stream: Stream): boolean {
  return stream.state === 'backfilling' || stream.state === 'recording' || stream.state === 'error'
}

/** Changes that still ask something of the operator. */
export function isOpen(change: Change): boolean {
  return change.state === 'provisional' || change.state === 'confirmed'
}

/** Jobs that can still be cancelled. */
export function isActive(job: Job): boolean {
  return job.state === 'queued' || job.state === 'running'
}

/** Open changes as the dashboard groups them. */
export interface ByDeadline {
  /** Still exportable: the nearest deadline first, then the versions still on air. */
  pending: Change[]
  /** Past their deadline, when the DVR may have dropped part of them, the latest first. */
  missed: Change[]
}

/** Splits open changes by whether their DVR deadline has passed (D-23). */
export function byDeadline(changes: readonly Change[], now: number): ByDeadline {
  const open = changes.filter(isOpen)
  const isMissed = (change: Change) =>
    change.dvr_deadline_at !== null && change.dvr_deadline_at <= now
  const soonest = (change: Change) => change.dvr_deadline_at ?? Number.POSITIVE_INFINITY
  return {
    pending: open.filter((change) => !isMissed(change)).sort((a, b) => soonest(a) - soonest(b)),
    missed: open
      .filter(isMissed)
      .sort((a, b) => (b.dvr_deadline_at ?? 0) - (a.dvr_deadline_at ?? 0)),
  }
}
