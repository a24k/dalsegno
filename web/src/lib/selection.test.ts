import { describe, expect, test } from 'vitest'
import { clipBody, defaultKind, isSelected, selectionSpan, suggestions, toggle } from './selection'
import type { Bar, BarRegion, Comparison } from './types'

function region(id: number, cls: BarRegion['class'], start: number, end: number): BarRegion {
  return { id, class: cls, start, end, match_start: null, video_boundaries: [] }
}

const NEW_BAR: Bar = {
  version_id: 4,
  start: 1_000,
  period: 1_000,
  regions: [
    region(1, 'new', 1_000, 1_100),
    region(2, 'revival', 1_100, 1_200),
    region(3, 'existing', 1_200, 1_500),
    region(4, 'known', 1_500, 1_600),
    region(5, 'deleted', 1_600, 1_700),
    region(6, 'new', 1_700, 2_000),
  ],
}

const OLD_BAR: Bar = { version_id: 3, start: 500, period: 1_200, regions: [] }

function compare(old: Bar | null, next: Bar | null): Comparison {
  return { change_id: 1, state: 'confirmed', old, new: next, checks: [] }
}

describe('toggle', () => {
  test('starts a selection with the first click', () => {
    expect(toggle(null, 'new', 2)).toEqual({ bar: 'new', first: 2, last: 2 })
  })

  test('grows a selection with a click on a neighbour, on either side', () => {
    const one = toggle(null, 'new', 2)

    const grown = toggle(toggle(one, 'new', 3), 'new', 1)

    expect(grown).toEqual({ bar: 'new', first: 1, last: 3 })
  })

  test('starts over with a click that is not adjacent', () => {
    expect(toggle({ bar: 'new', first: 1, last: 2 }, 'new', 5)).toEqual({
      bar: 'new',
      first: 5,
      last: 5,
    })
  })

  test('starts over with a click on the other bar', () => {
    expect(toggle({ bar: 'new', first: 1, last: 2 }, 'old', 1)).toEqual({
      bar: 'old',
      first: 1,
      last: 1,
    })
  })

  test('shrinks from an end, and clears when the last region is clicked again', () => {
    const shrunk = toggle({ bar: 'new', first: 1, last: 3 }, 'new', 3)
    const first = toggle({ bar: 'new', first: 1, last: 3 }, 'new', 1)

    expect(shrunk).toEqual({ bar: 'new', first: 1, last: 2 })
    expect(first).toEqual({ bar: 'new', first: 2, last: 3 })
    expect(toggle({ bar: 'new', first: 2, last: 2 }, 'new', 2)).toBeNull()
  })

  test('keeps the selection contiguous when a middle region is clicked', () => {
    expect(toggle({ bar: 'new', first: 1, last: 3 }, 'new', 2)).toEqual({
      bar: 'new',
      first: 2,
      last: 2,
    })
  })
})

describe('isSelected', () => {
  test('is true only for the selected regions of the selected bar', () => {
    const selection = { bar: 'new', first: 1, last: 2 } as const

    expect(isSelected(selection, 'new', 2)).toBe(true)
    expect(isSelected(selection, 'new', 3)).toBe(false)
    expect(isSelected(selection, 'old', 2)).toBe(false)
    expect(isSelected(null, 'new', 2)).toBe(false)
  })
})

describe('selectionSpan', () => {
  test('runs from the start of the first region to the end of the last', () => {
    expect(selectionSpan(NEW_BAR, { bar: 'new', first: 1, last: 3 })).toEqual({
      start: 1_100,
      end: 1_600,
    })
  })
})

describe('defaultKind', () => {
  test('is a cycle on the old bar and an addition on the new one', () => {
    expect([defaultKind('old'), defaultKind('new')]).toEqual(['cycle', 'addition'])
  })
})

describe('suggestions', () => {
  test('offers the whole old cycle and one addition for each run of added material', () => {
    const found = suggestions(compare(OLD_BAR, NEW_BAR), true)

    expect(found.map((item) => [item.kind, item.versionId, item.start, item.end])).toEqual([
      ['cycle', 3, 500, 1_700],
      ['addition', 4, 1_000, 1_200],
      ['addition', 4, 1_500, 1_600],
      ['addition', 4, 1_700, 2_000],
    ])
    expect(found[0].label).toBe('Old version, whole cycle')
    expect(found[1].label).toBe('Addition 1, 0:00:00 to 0:03:20')
  })

  test('leaves the whole cycle out once the change is closed', () => {
    const found = suggestions(compare(OLD_BAR, NEW_BAR), false)

    expect(found.every((item) => item.kind === 'addition')).toBe(true)
  })

  test('offers only the old cycle while there is no new version', () => {
    const found = suggestions(compare(OLD_BAR, null), true)

    expect(found.map((item) => item.kind)).toEqual(['cycle'])
  })

  test('offers nothing for a change with no bars', () => {
    expect(suggestions(compare(null, null), true)).toEqual([])
  })
})

describe('clipBody', () => {
  test('is the body POST /clips takes', () => {
    expect(clipBody(7, 'addition', 4, { start: 1_000, end: 1_200 })).toEqual({
      change: 7,
      kind: 'addition',
      start: 1_000,
      end: 1_200,
      version: 4,
    })
  })
})
