import { describe, expect, test } from 'vitest'
import { ago, holes, inJapan, percent, period, timeLeft } from './format'

// 2026-11-04 13:00 UTC, which is 22:00 in Japan.
const DEADLINE = Date.UTC(2026, 10, 4, 13, 0)
const HOUR = 3_600_000

describe('inJapan', () => {
  test('gives the moment in Japan time', () => {
    expect(inJapan(DEADLINE)).toBe('11/04 22:00 JST')
  })
})

describe('timeLeft', () => {
  test('says there is no deadline while the version is on air', () => {
    expect(timeLeft(null, DEADLINE)).toBe('No deadline while it is on air')
  })

  test('gives whole hours and the deadline', () => {
    expect(timeLeft(DEADLINE, DEADLINE - 48.5 * HOUR)).toBe(
      '48 hours left, export by 11/04 22:00 JST',
    )
  })

  test('gives minutes in the last hour', () => {
    expect(timeLeft(DEADLINE, DEADLINE - 0.25 * HOUR)).toBe(
      '15 min left, export by 11/04 22:00 JST',
    )
  })

  test('says when the deadline has passed', () => {
    expect(timeLeft(DEADLINE, DEADLINE + 1)).toBe('Past the deadline of 11/04 22:00 JST')
  })
})

describe('ago', () => {
  test('uses the largest whole unit', () => {
    expect(ago(null, DEADLINE)).toBe('never')
    expect(ago(DEADLINE - 10_000, DEADLINE)).toBe('just now')
    expect(ago(DEADLINE - 5 * 60_000, DEADLINE)).toBe('5 min ago')
    expect(ago(DEADLINE - 3 * HOUR, DEADLINE)).toBe('3 h ago')
    expect(ago(DEADLINE - 50 * HOUR, DEADLINE)).toBe('2 days ago')
  })
})

describe('numbers', () => {
  test('give a share, a period and holes in words', () => {
    expect(percent(null)).toBeNull()
    expect(percent(0.426)).toBe('43%')
    expect(period(null)).toBe('No period yet')
    expect(holes(12, 0)).toBe('12 to fill, 0 lost')
  })

  test('give a period in hours and minutes with the exact seconds', () => {
    expect(period(42244.8)).toBe('11 h 44 min (42244.8 s)')
    expect(period(7200)).toBe('2 h (7200.0 s)')
    expect(period(1800.04)).toBe('30 min (1800.0 s)')
    expect(period(12.5)).toBe('12.5 s')
  })
})
