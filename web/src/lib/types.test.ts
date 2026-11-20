import { describe, expect, test } from 'vitest'
import { byDeadline, canStart, canStop, type Change, type Stream } from './types'

const NOW = 1_000_000

function change(id: number, state: Change['state'], deadline: number | null): Change {
  return {
    id,
    program_id: 1,
    stream_id: 1,
    state,
    detected_at: 0,
    dvr_deadline_at: deadline,
    start_time: null,
  }
}

describe('byDeadline', () => {
  test('keeps exportable changes apart from those past their deadline', () => {
    const changes = [
      change(1, 'confirmed', NOW - 500),
      change(5, 'confirmed', NOW - 100),
      change(7, 'provisional', null),
      change(8, 'confirmed', NOW + 900),
      change(9, 'confirmed', NOW + 300),
    ]

    const { pending, missed } = byDeadline(changes, NOW)

    expect(pending.map((item) => item.id)).toEqual([9, 8, 7])
    expect(missed.map((item) => item.id)).toEqual([5, 1])
  })

  test('leaves out changes that are resolved or dismissed', () => {
    const changes = [change(2, 'resolved', NOW + 100), change(3, 'dismissed', NOW - 100)]

    expect(byDeadline(changes, NOW)).toEqual({ pending: [], missed: [] })
  })

  test('counts a deadline that is due now as missed', () => {
    expect(byDeadline([change(4, 'confirmed', NOW)], NOW).missed).toHaveLength(1)
  })
})

function stream(state: Stream['state']): Stream {
  return {
    id: 1,
    video_id: 'abcDEF123_-',
    title: 'Example TV',
    state,
    state_reason: null,
    period: null,
    follow_sq: null,
    last_fetch_at: null,
    open_fragments: 0,
    lost_fragments: 0,
    program_id: null,
  }
}

describe('canStart and canStop', () => {
  test('a paused stream can start and not stop', () => {
    expect(canStart(stream('paused'))).toBe(true)
    expect(canStop(stream('paused'))).toBe(false)
  })

  test('a recording stream can stop and not start', () => {
    expect(canStart(stream('recording'))).toBe(false)
    expect(canStop(stream('recording'))).toBe(true)
  })

  test('a failed stream can do either', () => {
    expect(canStart(stream('error'))).toBe(true)
    expect(canStop(stream('error'))).toBe(true)
  })

  test('an ended stream can do neither', () => {
    expect(canStart(stream('ended'))).toBe(false)
    expect(canStop(stream('ended'))).toBe(false)
  })
})
