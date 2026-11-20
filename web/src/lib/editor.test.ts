import { describe, expect, test } from 'vitest'
import {
  candidateRows,
  followDecision,
  fromPlayhead,
  nextKeyframe,
  placement,
  playheadStep,
  previousKeyframe,
  snapToKeyframe,
  span,
  timeAtRatio,
  timecode,
  toStreamTime,
  toVideoTime,
} from './editor'
import type { EdgeReport, Job } from './types'

const KEYFRAMES = [
  { sq: 12, time: 120 },
  { sq: 10, time: 100 },
  { sq: 11, time: 110 },
]
const WINDOW = { start: 100, end: 140 }

const REPORT: EdgeReport = {
  around: 120,
  window: WINDOW,
  preview_start: 99.5,
  keyframes: KEYFRAMES,
  black: [],
  silence: [],
  starts: [{ keyframe: KEYFRAMES[1], evidence: 'both', interval: { start: 100, end: 101 } }],
  ends: [{ time: 130.5, edge: 'sound_returns', evidence: 'silence_only' }],
  relaxed: false,
}

describe('snapToKeyframe', () => {
  test('returns the keyframe at or before the time', () => {
    expect(snapToKeyframe(KEYFRAMES, 115)?.sq).toBe(11)
    expect(snapToKeyframe(KEYFRAMES, 120)?.sq).toBe(12)
  })

  test('returns null when every keyframe is later', () => {
    expect(snapToKeyframe(KEYFRAMES, 99)).toBeNull()
  })
})

describe('previousKeyframe and nextKeyframe', () => {
  test('find the neighbours of a time between keyframes', () => {
    expect(previousKeyframe(KEYFRAMES, 115)?.sq).toBe(11)
    expect(nextKeyframe(KEYFRAMES, 115)?.sq).toBe(12)
  })

  test('skip the keyframe the playhead is on', () => {
    expect(previousKeyframe(KEYFRAMES, 110)?.sq).toBe(10)
    expect(nextKeyframe(KEYFRAMES, 110)?.sq).toBe(12)
  })

  test('return null at the ends', () => {
    expect(previousKeyframe(KEYFRAMES, 100)).toBeNull()
    expect(nextKeyframe(KEYFRAMES, 120)).toBeNull()
  })
})

describe('placement', () => {
  test('gives the percentage of the window', () => {
    expect(placement(WINDOW, 110)).toBe(25)
  })

  test('is null outside the window', () => {
    expect(placement(WINDOW, 99)).toBeNull()
    expect(placement(WINDOW, 141)).toBeNull()
  })
})

describe('span', () => {
  test('gives the left edge and width of an interval', () => {
    expect(span(WINDOW, { start: 110, end: 120 })).toEqual({ left: 25, width: 25 })
  })

  test('cuts an interval to the window and drops one outside it', () => {
    expect(span(WINDOW, { start: 90, end: 110 })).toEqual({ left: 0, width: 25 })
    expect(span(WINDOW, { start: 150, end: 160 })).toBeNull()
  })
})

describe('timeAtRatio', () => {
  test('maps a click on the track to stream time and clamps it', () => {
    expect(timeAtRatio(WINDOW, 0.5)).toBe(120)
    expect(timeAtRatio(WINDOW, 2)).toBe(140)
    expect(timeAtRatio(WINDOW, -1)).toBe(100)
  })
})

describe('video time', () => {
  test('converts both ways against the preview start', () => {
    expect(toVideoTime(REPORT, 110)).toBe(10.5)
    expect(toStreamTime(REPORT, 10.5)).toBe(110)
  })
})

describe('timecode', () => {
  test('writes h:mm:ss.sss', () => {
    expect(timecode(3723.4)).toBe('1:02:03.400')
    expect(timecode(0)).toBe('0:00:00.000')
  })

  test('marks a time before the window start', () => {
    expect(timecode(-1.5)).toBe('-0:00:01.500')
  })
})

describe('playheadStep', () => {
  test('moves 0.1 s, or 1 s with Shift, and ignores other keys', () => {
    expect(playheadStep('ArrowRight', false)).toBe(0.1)
    expect(playheadStep('ArrowLeft', true)).toBe(-1)
    expect(playheadStep('a', false)).toBeNull()
  })
})

describe('fromPlayhead', () => {
  test('snaps a start to the keyframe before and keeps an end exact', () => {
    expect(fromPlayhead('start', KEYFRAMES, 115.5)).toBe(110)
    expect(fromPlayhead('end', KEYFRAMES, 115.5)).toBe(115.5)
  })

  test('keeps a start as it is when no keyframe is before', () => {
    expect(fromPlayhead('start', KEYFRAMES, 90)).toBe(90)
  })
})

describe('candidateRows', () => {
  test('describes the starts with their keyframe and evidence', () => {
    expect(candidateRows('start', REPORT)).toEqual([
      { key: 'start-0', time: 100, detail: 'keyframe sq 10, black and silence' },
    ])
  })

  test('describes the ends with their edge and evidence', () => {
    expect(candidateRows('end', REPORT)).toEqual([
      { key: 'end-0', time: 130.5, detail: 'sound returns, silence only' },
    ])
  })
})

describe('followDecision', () => {
  const job = (state: Job['state']): Job => ({
    id: 4,
    kind: 'candidates',
    target: { clip: 1 },
    state,
    progress: null,
    error: null,
    created_at: 0,
    started_at: null,
    finished_at: null,
  })

  test('is ready when the answer carries no active job', () => {
    expect(followDecision(false, null)).toBe('ready')
    expect(followDecision(true, job('succeeded'))).toBe('ready')
  })

  test('follows an active job the first time', () => {
    expect(followDecision(false, job('queued'))).toBe('following')
  })

  test('stalls when a job is active again after one succeeded', () => {
    expect(followDecision(true, job('running'))).toBe('stalled')
  })
})
