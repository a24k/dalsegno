// The screens and their routes, as docs/design/api.md lists them.

export type Screen =
  | 'dashboard'
  | 'streams'
  | 'stream'
  | 'change'
  | 'clip-edit'
  | 'clip-export'
  | 'jobs'
  | 'settings'
  | 'not-found'

export interface Route {
  screen: Screen
  params: Record<string, string>
}

const PATTERNS: ReadonlyArray<readonly [string, Screen]> = [
  ['/', 'dashboard'],
  ['/streams', 'streams'],
  ['/streams/:id', 'stream'],
  ['/changes/:id', 'change'],
  ['/clips/:id/edit', 'clip-edit'],
  ['/clips/:id/export', 'clip-export'],
  ['/jobs', 'jobs'],
  ['/settings', 'settings'],
]

function segments(path: string): string[] {
  return path.split('/').filter((segment) => segment !== '')
}

function decode(segment: string): string | null {
  try {
    return decodeURIComponent(segment)
  } catch {
    return null
  }
}

function matchPattern(pattern: string, path: string): Record<string, string> | null {
  const wanted = segments(pattern)
  const given = segments(path)
  if (wanted.length !== given.length) {
    return null
  }
  const params: Record<string, string> = {}
  for (const [index, part] of wanted.entries()) {
    const value = given[index]
    if (part.startsWith(':')) {
      const decoded = decode(value)
      if (decoded === null) {
        return null
      }
      params[part.slice(1)] = decoded
    } else if (part !== value) {
      return null
    }
  }
  return params
}

/** Finds the screen for a path. A trailing slash does not matter. */
export function resolve(path: string): Route {
  for (const [pattern, screen] of PATTERNS) {
    const params = matchPattern(pattern, path)
    if (params !== null) {
      return { screen, params }
    }
  }
  return { screen: 'not-found', params: {} }
}
