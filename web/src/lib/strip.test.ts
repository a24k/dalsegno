import { describe, expect, test } from 'vitest'
import {
  axisLength,
  boundaryLabel,
  isWide,
  place,
  placePoint,
  regionLabel,
  regionName,
  ticks,
} from './strip'
import type { Bar, BarRegion, Comparison } from './types'

function region(id: number, cls: BarRegion['class'], start: number, end: number): BarRegion {
  return { id, class: cls, start, end, match_start: null, video_boundaries: [] }
}

const NEW_BAR: Bar = {
  version_id: 4,
  start: 10_000,
  period: 1_800,
  regions: [region(1, 'existing', 10_000, 10_900), region(2, 'new', 10_900, 11_650)],
}

const OLD_BAR: Bar = { version_id: 3, start: 5_000, period: 3_600, regions: [] }

describe('axisLength', () => {
  test('is the longer of the two periods', () => {
    const comparison = { old: OLD_BAR, new: NEW_BAR } as Comparison

    expect(axisLength(comparison)).toBe(3_600)
  })

  test('is the period of the only bar when there is no new version', () => {
    const comparison = { old: OLD_BAR, new: null } as Comparison

    expect(axisLength(comparison)).toBe(3_600)
  })

  test('is zero when there is no bar', () => {
    expect(axisLength({ old: null, new: null } as Comparison)).toBe(0)
  })
})

describe('place', () => {
  test('puts a region by its distance from the bar start on the shared axis', () => {
    const placement = place(NEW_BAR, NEW_BAR.regions[1], 3_600)

    expect(placement.left).toBeCloseTo(25)
    expect(placement.width).toBeCloseTo(750 / 36)
  })

  test('keeps a region inside the axis', () => {
    const wide = region(9, 'new', 10_000, 20_000)

    const placement = place(NEW_BAR, wide, 1_800)

    expect(placement).toEqual({ left: 0, width: 100 })
  })

  test('puts nothing anywhere on an empty axis', () => {
    expect(place(NEW_BAR, NEW_BAR.regions[0], 0)).toEqual({ left: 0, width: 0 })
  })
})

describe('placePoint', () => {
  test('puts a video boundary by its distance from the bar start', () => {
    expect(placePoint(NEW_BAR, 10_450, 1_800)).toBeCloseTo(25)
  })
})

describe('isWide', () => {
  test('keeps the label in the title when the block is narrow', () => {
    expect(isWide({ left: 0, width: 4 })).toBe(false)
    expect(isWide({ left: 0, width: 30 })).toBe(true)
  })
})

describe('labels', () => {
  test('gives the class and the length', () => {
    expect(regionLabel(NEW_BAR.regions[1])).toBe('new 0:12:30')
  })

  test('writes a class with an underscore as words', () => {
    expect(regionLabel(region(3, 'no_data', 0, 60))).toBe('no data 0:01:00')
  })

  test('names a region by its span from the bar start', () => {
    expect(regionName(NEW_BAR, NEW_BAR.regions[1])).toBe('new, 0:15:00 to 0:27:30, 0:12:30')
  })

  test('names a video boundary from the bar start', () => {
    expect(boundaryLabel(NEW_BAR, 10_310)).toBe('video boundary at 0:05:10')
  })
})

describe('ticks', () => {
  test('marks each minute and labels few enough to read', () => {
    const marks = ticks(3_600)

    expect(marks).toHaveLength(61)
    expect(marks[0]).toEqual({ left: 0, label: '0:00:00' })
    expect(marks[1].label).toBeNull()
    expect(marks[5].label).toBe('0:05:00')
    expect(marks[60].left).toBe(100)
  })

  test('labels every minute on a short axis', () => {
    expect(ticks(300).every((tick) => tick.label !== null)).toBe(true)
  })
})
