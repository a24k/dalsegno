<script lang="ts">
  import Placeholder from './lib/Placeholder.svelte'
  import Change from './screens/Change.svelte'
  import Dashboard from './screens/Dashboard.svelte'
  import Jobs from './screens/Jobs.svelte'
  import Settings from './screens/Settings.svelte'
  import Stream from './screens/Stream.svelte'
  import Streams from './screens/Streams.svelte'
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
  <div class="identity">
    <a class="brand" href="/">dalsegno</a>
    {#if live.version}<span class="version">{live.version}</span>{/if}
  </div>
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
    {#if router.route.screen === 'dashboard'}
      <Dashboard />
    {:else if router.route.screen === 'streams'}
      <Streams />
    {:else if router.route.screen === 'stream'}
      <Stream />
    {:else if router.route.screen === 'change'}
      <Change />
    {:else if router.route.screen === 'jobs'}
      <Jobs />
    {:else if router.route.screen === 'settings'}
      <Settings />
    {:else}
      <Placeholder {title} />
    {/if}
  {/key}
</main>
