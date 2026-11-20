// Words and numbers for the screens. Deadlines are in Japan time, as the notifications give
// them (docs/design/api.md).

const MINUTE_MS = 60_000
const HOUR_MS = 60 * MINUTE_MS
const DAY_MS = 24 * HOUR_MS

const JAPAN = new Intl.DateTimeFormat('en-GB', {
  timeZone: 'Asia/Tokyo',
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
  hour12: false,
})

/** A moment in Japan time, such as `11/04 22:00 JST`. */
export function inJapan(ms: number): string {
  const parts = Object.fromEntries(JAPAN.formatToParts(ms).map((part) => [part.type, part.value]))
  return `${parts.month}/${parts.day} ${parts.hour}:${parts.minute} JST`
}

/**
 * The time left to export a change (D-23). A change with no deadline belongs to a version
 * still on air, whose sections come round again.
 */
export function timeLeft(deadline: number | null, now: number): string {
  if (deadline === null) {
    return 'No deadline while it is on air'
  }
  const left = deadline - now
  if (left <= 0) {
    return `Past the deadline of ${inJapan(deadline)}`
  }
  const amount =
    left < HOUR_MS
      ? `${Math.max(1, Math.floor(left / MINUTE_MS))} min`
      : `${Math.floor(left / HOUR_MS)} hours`
  return `${amount} left, export by ${inJapan(deadline)}`
}

/** How long ago something happened, in the largest whole unit. */
export function ago(ms: number | null, now: number): string {
  if (ms === null) {
    return 'never'
  }
  const elapsed = Math.max(0, now - ms)
  if (elapsed < MINUTE_MS) {
    return 'just now'
  }
  if (elapsed < HOUR_MS) {
    return `${Math.floor(elapsed / MINUTE_MS)} min ago`
  }
  if (elapsed < DAY_MS) {
    return `${Math.floor(elapsed / HOUR_MS)} h ago`
  }
  return `${Math.floor(elapsed / DAY_MS)} days ago`
}

/** A share done as a whole percentage, or null before a job reports any. */
export function percent(progress: number | null): string | null {
  return progress === null ? null : `${Math.round(progress * 100)}%`
}

/** A loop period in hours and minutes with the exact seconds, or that none is known yet. */
export function period(seconds: number | null): string {
  if (seconds === null) {
    return 'No period yet'
  }
  const exact = `${seconds.toFixed(1)} s`
  const minutes = Math.round(seconds / 60)
  if (minutes < 1) {
    return exact
  }
  const hours = Math.floor(minutes / 60)
  const rest = minutes % 60
  const rounded = hours === 0 ? `${rest} min` : rest === 0 ? `${hours} h` : `${hours} h ${rest} min`
  return `${rounded} (${exact})`
}

/** A stream's holes, as `dalsegno status` gives them. */
export function holes(open: number, lost: number): string {
  return `${open} to fill, ${lost} lost`
}
