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
  old_version_id: number | null
  new_version_id: number | null
}

/** What a change's regions are classed as. */
export type RegionClass = 'existing' | 'new' | 'revival' | 'known' | 'deleted' | 'no_data'

/** A region of a change, as `GET /changes/{id}` lists it. */
export interface Region {
  id: number
  class: RegionClass
  start: number
  end: number
  match_start: number | null
  match_version: number | null
}

export type ClipKind = 'cycle' | 'addition'

/** A clip, from the moment its regions are chosen until it is exported. */
export interface Clip {
  id: number
  state: 'draft' | 'exported'
  kind: ClipKind
  change_id: number | null
  version_id: number | null
  stream_id: number
  start: number | null
  end: number | null
  name: string | null
  duration: number | null
  output_path: string | null
  plex_status: 'none' | 'requested' | 'confirmed' | 'failed'
}

/** A change with its regions and the clips made from it. */
export interface ChangeDetail extends Change {
  regions: Region[]
  clips: Clip[]
}

/** A region on one bar of the version strip. Times are stream seconds. */
export interface BarRegion {
  id: number
  class: RegionClass
  start: number
  end: number
  match_start: number | null
  video_boundaries: number[]
}

/** One version of a change, from the start of its cycle. */
export interface Bar {
  version_id: number
  start: number
  period: number
  regions: BarRegion[]
}

/** One automatic check of a change. */
export interface SelfCheck {
  name: string
  expected: number
  actual: number
  tolerance: number
  passed: boolean
}

/** Both versions of a change, as `GET /changes/{id}/diff` gives them. */
export interface Comparison {
  change_id: number
  state: Change['state']
  old: Bar | null
  new: Bar | null
  checks: SelfCheck[]
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

/** A stretch of stream time, in seconds. */
export interface Interval {
  start: number
  end: number
}

/** A fragment start, which is a keyframe. */
export interface Keyframe {
  sq: number
  time: number
}

/** What a boundary candidate rests on. */
export type Evidence = 'both' | 'black_only' | 'silence_only'

/** A keyframe a cut can start from. */
export interface StartCandidate {
  keyframe: Keyframe
  evidence: Evidence
  interval: Interval
}

/** A time a cut can end at, on one edge of a silence. */
export interface EndCandidate {
  time: number
  edge: 'sound_stops' | 'sound_returns'
  evidence: Evidence
}

/** What the candidates job found around one edge of a clip. Times are stream seconds. */
export interface EdgeReport {
  around: number
  /** The stream time the preview and the sheet cover. */
  window: Interval
  /** The stream time of the preview's first frame. */
  preview_start: number
  keyframes: Keyframe[]
  black: Interval[]
  silence: Interval[]
  starts: StartCandidate[]
  ends: EndCandidate[]
  relaxed: boolean
}

/** The period of a cycle clip as measured from black at its start. */
export interface RefinedPeriod {
  period: number
  agreeing: number
}

export interface MeasuredPeriod {
  start: number
  audio: number
  video: RefinedPeriod | null
}

/** What `GET /clips/{id}/candidates` answers, with the job when it is still being found. */
export interface ClipCandidates {
  clip: Clip
  start: EdgeReport | null
  end: EdgeReport | null
  period: MeasuredPeriod | null
  job: Job | null
}

export type Edge = 'start' | 'end'
