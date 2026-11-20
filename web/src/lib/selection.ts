// Choosing regions on the strip, and the clips the screen suggests.

import { clock } from './format'
import type { Bar, BarRegion, ClipKind, Comparison } from './types'

export type BarName = 'old' | 'new'

/** A run of neighbouring regions on one bar, by their place in the bar. */
export interface Selection {
  bar: BarName
  first: number
  last: number
}

/**
 * The selection after a click on a region. A click next to the selection grows it, a click
 * on an end of it shrinks it, and any other click starts a new selection, as does a click on
 * the other bar.
 */
export function toggle(selection: Selection | null, bar: BarName, index: number): Selection | null {
  if (selection === null || selection.bar !== bar) {
    return { bar, first: index, last: index }
  }
  const { first, last } = selection
  if (index === first && index === last) {
    return null
  }
  if (index === first) {
    return { bar, first: first + 1, last }
  }
  if (index === last) {
    return { bar, first, last: last - 1 }
  }
  if (index === first - 1) {
    return { bar, first: index, last }
  }
  if (index === last + 1) {
    return { bar, first, last: index }
  }
  return { bar, first: index, last: index }
}

/** Whether a region is in the selection. */
export function isSelected(selection: Selection | null, bar: BarName, index: number): boolean {
  return (
    selection !== null &&
    selection.bar === bar &&
    index >= selection.first &&
    index <= selection.last
  )
}

/** The stretch of stream time a selection covers. */
export interface Span {
  start: number
  end: number
}

/** From the start of the first region to the end of the last. */
export function selectionSpan(bar: Bar, selection: Selection): Span {
  const first = bar.regions[selection.first]
  const last = bar.regions[selection.last]
  return { start: first.start, end: last.end }
}

/** The kind a clip from this bar starts as. */
export function defaultKind(bar: BarName): ClipKind {
  return bar === 'old' ? 'cycle' : 'addition'
}

/** A clip the screen offers to make. */
export interface Candidate extends Span {
  label: string
  kind: ClipKind
  versionId: number
}

/** Regions of new material, which an addition is made of. */
const ADDED = new Set(['new', 'revival', 'known'])

function runs(regions: readonly BarRegion[]): BarRegion[][] {
  const found: BarRegion[][] = []
  let current: BarRegion[] = []
  for (const region of regions) {
    if (ADDED.has(region.class)) {
      current.push(region)
    } else if (current.length > 0) {
      found.push(current)
      current = []
    }
  }
  return current.length > 0 ? [...found, current] : found
}

/**
 * The clips worth making: the old version's whole cycle while the change is open, and an
 * addition for each run of neighbouring new, revival or known regions on the new bar.
 */
export function suggestions(comparison: Comparison, isOpen: boolean): Candidate[] {
  const whole: Candidate[] =
    comparison.old !== null && isOpen
      ? [
          {
            label: 'Old version, whole cycle',
            kind: 'cycle',
            versionId: comparison.old.version_id,
            start: comparison.old.start,
            end: comparison.old.start + comparison.old.period,
          },
        ]
      : []
  const bar = comparison.new
  if (bar === null) {
    return whole
  }
  const additions = runs(bar.regions).map((run, index) => {
    const start = run[0].start
    const end = run[run.length - 1].end
    return {
      label: `Addition ${index + 1}, ${clock(start - bar.start)} to ${clock(end - bar.start)}`,
      kind: 'addition' as const,
      versionId: bar.version_id,
      start,
      end,
    }
  })
  return [...whole, ...additions]
}

/** The body of `POST /clips`. */
export function clipBody(
  changeId: number,
  kind: ClipKind,
  versionId: number,
  span: Span,
): Record<string, unknown> {
  return { change: changeId, kind, start: span.start, end: span.end, version: versionId }
}
