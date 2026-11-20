<script lang="ts">
  import { clock } from '../../lib/format'
  import type { Clip } from '../../lib/types'

  let { clips }: { clips: Clip[] } = $props()

  function length(clip: Clip): string {
    return clip.start === null || clip.end === null ? '' : clock(clip.end - clip.start)
  }

  function span(clip: Clip): string {
    return clip.start === null || clip.end === null
      ? 'No span'
      : `${clip.start.toFixed(1)} s to ${clip.end.toFixed(1)} s`
  }
</script>

<section aria-labelledby="clips-heading">
  <h2 id="clips-heading">Clips of this change</h2>
  {#if clips.length === 0}
    <p>No clip has been made yet.</p>
  {:else}
    <table>
      <thead>
        <tr>
          <th scope="col">Clip</th>
          <th scope="col">Kind</th>
          <th scope="col">Span</th>
          <th scope="col">Length</th>
          <th scope="col">State</th>
          <th scope="col">Edit</th>
        </tr>
      </thead>
      <tbody>
        {#each clips as clip (clip.id)}
          <tr>
            <th scope="row">{clip.name ?? `Clip ${clip.id}`}</th>
            <td>{clip.kind}</td>
            <td>{span(clip)}</td>
            <td>{length(clip)}</td>
            <td><span class="state" data-state={clip.state}>{clip.state}</span></td>
            <td><a href="/clips/{clip.id}/edit">Edit boundaries</a></td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>
