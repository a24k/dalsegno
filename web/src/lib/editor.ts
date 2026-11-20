// Pure helpers of the boundary editor: keyframes, seek-bar placement and times.

import {
  isActive,
  type EdgeReport,
  type Evidence,
  type Interval,
  type Job,
  type Keyframe,
} from './types'

/** Seconds the arrow keys move the playhead, and with Shift. */
export const STEP_SECONDS = 0.1
export const BIG_STEP_SECONDS = 1

/** Slack when comparing times, so the playhead on a keyframe is not before it. */
const EPSILON = 0.001

/** The keyframe at or before `time`, or null when none is that early. */
export function snapToKeyframe(keyframes: readonly Keyframe[], time: number): Keyframe | null {
  let found: Keyframe | null = null
  for (const keyframe of keyframes) {
    if (keyframe.time <= time + EPSILON && (found === null || keyframe.time > found.time)) {
      found = keyframe
    }
  }
  return found
}

/** The latest keyframe before `time`. */
export function previousKeyframe(keyframes: readonly Keyframe[], time: number): Keyframe | null {
  let found: Keyframe | null = null
  for (const keyframe of keyframes) {
    if (keyframe.time < time - EPSILON && (found === null || keyframe.time > found.time)) {
      found = keyframe
    }
  }
  return found
}

/** The earliest keyframe after `time`. */
export function nextKeyframe(keyframes: readonly Keyframe[], time: number): Keyframe | null {
  let found: Keyframe | null = null
  for (const keyframe of keyframes) {
    if (keyframe.time > time + EPSILON && (found === null || keyframe.time < found.time)) {
      found = keyframe
    }
  }
  return found
}

/** Where a time lies on a track that shows `window`, as a percentage, or null outside it. */
export function placement(window: Interval, time: number): number | null {
  const length = window.end - window.start
  if (length <= 0 || time < window.start || time > window.end) {
    return null
  }
  return ((time - window.start) / length) * 100
}

/** The left edge and width of an interval on the track, in percent, cut to the window. */
export function span(window: Interval, interval: Interval): { left: number; width: number } | null {
  const length = window.end - window.start
  const start = Math.max(interval.start, window.start)
  const end = Math.min(interval.end, window.end)
  if (length <= 0 || end <= start) {
    return null
  }
  return { left: ((start - window.start) / length) * 100, width: ((end - start) / length) * 100 }
}

/** The stream time under a click at `ratio` (0 to 1) of the track. */
export function timeAtRatio(window: Interval, ratio: number): number {
  const clamped = Math.min(1, Math.max(0, ratio))
  return window.start + clamped * (window.end - window.start)
}

/** Seconds into the preview for a stream time. */
export function toVideoTime(report: Pick<EdgeReport, 'preview_start'>, streamTime: number): number {
  return streamTime - report.preview_start
}

/** The stream time of a position in the preview. */
export function toStreamTime(report: Pick<EdgeReport, 'preview_start'>, videoTime: number): number {
  return report.preview_start + videoTime
}

/** A time as `h:mm:ss.sss`, with a minus sign before the window start. */
export function timecode(seconds: number): string {
  const sign = seconds < 0 ? '-' : ''
  const millis = Math.round(Math.abs(seconds) * 1000)
  const hours = Math.floor(millis / 3_600_000)
  const minutes = Math.floor((millis % 3_600_000) / 60_000)
  const rest = (millis % 60_000) / 1000
  return `${sign}${hours}:${String(minutes).padStart(2, '0')}:${rest.toFixed(3).padStart(6, '0')}`
}

/** How far an arrow key moves the playhead, or null for any other key. */
export function playheadStep(key: string, isShift: boolean): number | null {
  const size = isShift ? BIG_STEP_SECONDS : STEP_SECONDS
  if (key === 'ArrowLeft') {
    return -size
  }
  return key === 'ArrowRight' ? size : null
}

/** What an edge becomes when set to the playhead: a start snaps to a keyframe, an end is exact. */
export function fromPlayhead(
  edge: 'start' | 'end',
  keyframes: readonly Keyframe[],
  time: number,
): number {
  return edge === 'end' ? time : (snapToKeyframe(keyframes, time)?.time ?? time)
}

const EVIDENCE: Record<Evidence, string> = {
  both: 'black and silence',
  black_only: 'black only',
  silence_only: 'silence only',
}

/** The words for what a candidate rests on. */
export function evidenceWords(evidence: Evidence): string {
  return EVIDENCE[evidence]
}

/** One row of a panel's candidate list. */
export interface CandidateRow {
  key: string
  time: number
  detail: string
}

/** The candidates of an edge, best first as the service ranks them. */
export function candidateRows(edge: 'start' | 'end', report: EdgeReport): CandidateRow[] {
  if (edge === 'start') {
    return report.starts.map((candidate, index) => ({
      key: `start-${index}`,
      time: candidate.keyframe.time,
      detail: `keyframe sq ${candidate.keyframe.sq}, ${evidenceWords(candidate.evidence)}`,
    }))
  }
  return report.ends.map((candidate, index) => ({
    key: `end-${index}`,
    time: candidate.time,
    detail: `${candidate.edge === 'sound_stops' ? 'sound stops' : 'sound returns'}, ${evidenceWords(candidate.evidence)}`,
  }))
}

/** What the editor does with the job an answer carries. */
export type FollowDecision = 'ready' | 'following' | 'stalled'

/**
 * Decides whether to follow the job of an answer. After a job for this clip succeeded and the
 * candidates were read again, an active job means they are still not ready: stop following.
 */
export function followDecision(hasFinishedJob: boolean, job: Job | null): FollowDecision {
  if (job === null || !isActive(job)) {
    return 'ready'
  }
  return hasFinishedJob ? 'stalled' : 'following'
}
