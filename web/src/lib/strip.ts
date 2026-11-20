// Where the regions of a change go on the version strip, and the words for them.

import { clock } from './format'
import type { Bar, BarRegion, Comparison, RegionClass } from './types'

/** A region narrower than this share of the axis keeps its label in the `title` only. */
const WIDE_PERCENT = 9
const MINUTE = 60
/** The most labelled ticks the axis shows; the rest are plain marks. */
const MOST_LABELS = 12

/** How far a block sits along the axis and how wide it is, both in percent. */
export interface Placement {
  left: number
  width: number
}

/** The length of the axis: the longer of the two periods. */
export function axisLength(comparison: Comparison): number {
  return Math.max(comparison.old?.period ?? 0, comparison.new?.period ?? 0)
}

function percentOf(seconds: number, axis: number): number {
  return axis > 0 ? Math.min(100, Math.max(0, (seconds / axis) * 100)) : 0
}

/** Where a region goes on its bar, by its distance from the bar's start. */
export function place(bar: Bar, region: BarRegion, axis: number): Placement {
  const left = percentOf(region.start - bar.start, axis)
  const right = percentOf(region.end - bar.start, axis)
  return { left, width: right - left }
}

/** Where a point inside a bar, such as a video boundary, goes on the axis. */
export function placePoint(bar: Bar, time: number, axis: number): number {
  return percentOf(time - bar.start, axis)
}

/** Whether a block is wide enough to hold its label. */
export function isWide(placement: Placement): boolean {
  return placement.width >= WIDE_PERCENT
}

/** A tick on the axis: its place in percent, and its label when it has one. */
export interface Tick {
  left: number
  label: string | null
}

/** A tick for each minute, labelled often enough to read and not more. */
export function ticks(axis: number): Tick[] {
  const minutes = Math.floor(axis / MINUTE)
  const every = Math.max(1, Math.ceil(minutes / MOST_LABELS))
  return Array.from({ length: minutes + 1 }, (_, minute) => ({
    left: percentOf(minute * MINUTE, axis),
    label: minute % every === 0 ? clock(minute * MINUTE) : null,
  }))
}

/** A class as a phrase: `no_data` is `no data`. */
export function className(value: RegionClass): string {
  return value.replace('_', ' ')
}

/** The length of a region, as `h:mm:ss`. */
function length(region: BarRegion): string {
  return clock(region.end - region.start)
}

/** What a block shows when it is wide enough: its class and length. */
export function regionLabel(region: BarRegion): string {
  return `${className(region.class)} ${length(region)}`
}

/** The full name of a region, for its `title` and accessible name, from its bar's start. */
export function regionName(bar: Bar, region: BarRegion): string {
  const from = clock(region.start - bar.start)
  const to = clock(region.end - bar.start)
  return `${className(region.class)}, ${from} to ${to}, ${length(region)}`
}

/** The accessible label of a video boundary, from its bar's start. */
export function boundaryLabel(bar: Bar, time: number): string {
  return `video boundary at ${clock(time - bar.start)}`
}
