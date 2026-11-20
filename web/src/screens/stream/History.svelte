<script lang="ts">
  import { ago } from '../../lib/format'
  import type { Change, Version } from '../../lib/types'

  let { versions, changes, now }: { versions: Version[]; changes: Change[]; now: number } = $props()

  function stretch(version: Version): string {
    const from = `${version.on_air_from.toFixed(1)} s`
    return version.on_air_until === null
      ? `${from} on`
      : `${from} to ${version.on_air_until.toFixed(1)} s`
  }
</script>

<section aria-labelledby="versions-heading">
  <h2 id="versions-heading">Versions</h2>
  {#if versions.length === 0}
    <p>No version has been seen yet.</p>
  {:else}
    <table>
      <thead>
        <tr>
          <th scope="col">Version</th>
          <th scope="col">Period</th>
          <th scope="col">On air</th>
          <th scope="col">First seen</th>
        </tr>
      </thead>
      <tbody>
        {#each versions as version (version.id)}
          <tr>
            <th scope="row">Version {version.id}</th>
            <td>{version.period_audio.toFixed(1)} s</td>
            <td>{stretch(version)}</td>
            <td>{ago(version.first_seen_at, now)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<section aria-labelledby="changes-heading">
  <h2 id="changes-heading">Changes</h2>
  {#if changes.length === 0}
    <p>No change has been found yet.</p>
  {:else}
    <ul>
      {#each changes as change (change.id)}
        <li>
          <a href="/changes/{change.id}">Change {change.id}</a>
          <span class="state" data-state={change.state}>{change.state}</span>
          <span class="muted">{ago(change.detected_at, now)}</span>
        </li>
      {/each}
    </ul>
  {/if}
</section>
