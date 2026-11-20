// Path routing over the History API, so notification links open the right screen.

import { resolve, type Route } from './routes'

let path = $state(window.location.pathname)
const route = $derived<Route>(resolve(path))

window.addEventListener('popstate', () => {
  path = window.location.pathname
})

export const router = {
  get path(): string {
    return path
  },
  get route(): Route {
    return route
  },
}

/** Opens a path without reloading the page. */
export function navigate(to: string): void {
  if (to !== window.location.pathname) {
    window.history.pushState(null, '', to)
  }
  path = to
}

/**
 * Handles a click on a link to this service in the page, and leaves new tabs, downloads and
 * other sites to the browser.
 */
export function followLink(event: MouseEvent): void {
  const isPlainClick =
    event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey
  if (!isPlainClick || event.defaultPrevented) {
    return
  }
  const link = (event.target as Element | null)?.closest('a')
  if (!link || link.target || link.hasAttribute('download')) {
    return
  }
  if (link.origin !== window.location.origin || link.pathname.startsWith('/api/')) {
    return
  }
  event.preventDefault()
  navigate(link.pathname)
}
