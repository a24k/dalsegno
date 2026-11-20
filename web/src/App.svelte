<script lang="ts">
  import Placeholder from './lib/Placeholder.svelte'
  import { live } from './lib/live.svelte'
  import { followLink, router } from './lib/router.svelte'
  import type { Screen } from './lib/routes'

  const NAVIGATION: ReadonlyArray<readonly [string, string]> = [
    ['/', 'Dashboard'],
    ['/streams', 'Streams'],
    ['/jobs', 'Jobs'],
    ['/settings', 'Settings'],
  ]

  const TITLES: Record<Screen, string> = {
    dashboard: 'Dashboard',
    streams: 'Streams',
    stream: 'Stream',
    change: 'Change',
    'clip-edit': 'Edit boundaries',
    'clip-export': 'Export',
    jobs: 'Jobs',
    settings: 'Settings and diagnostics',
    'not-found': 'Not found',
  }

  let title = $derived(TITLES[router.route.screen])

  function isCurrent(path: string): boolean {
    if (router.route.screen === 'not-found') {
      return false
    }
    return path === '/' ? router.path === '/' : router.path.startsWith(path)
  }
</script>

<svelte:head>
  <title>{title} · dalsegno</title>
</svelte:head>

<svelte:document onclick={followLink} />

<a class="skip" href="#main">Skip to content</a>
<header>
  <a class="brand" href="/">dalsegno</a>
  <nav aria-label="Main">
    <ul>
      {#each NAVIGATION as [path, label] (path)}
        <li>
          <a href={path} aria-current={isCurrent(path) ? 'page' : undefined}>{label}</a>
        </li>
      {/each}
    </ul>
  </nav>
  <p class="live" role="status" data-connected={live.isConnected}>
    {live.isConnected ? 'Live updates on' : 'Live updates paused, reconnecting'}
  </p>
</header>
<main id="main" tabindex="-1">
  {#key router.path}
    <Placeholder {title} />
  {/key}
</main>
