<script lang="ts">
  import { candidateRows, timecode } from '../../lib/editor'
  import type { Edge, EdgeReport } from '../../lib/types'

  let {
    edge,
    report,
    readOnly,
    use,
  }: { edge: Edge; report: EdgeReport; readOnly: boolean; use: (time: number) => void } = $props()

  let rows = $derived(candidateRows(edge, report))
</script>

<h4>Candidates</h4>
<p>
  {rows.length === 1 ? '1 candidate' : `${rows.length} candidates`} found{report.relaxed
    ? ', with detection relaxed'
    : ''}. None is chosen for you.
</p>
{#if rows.length > 0}
  <ul>
    {#each rows as row (row.key)}
      <li>
        {timecode(row.time - report.window.start)} ({row.time.toFixed(3)} s), {row.detail}
        {#if !readOnly}
          <button type="button" onclick={() => use(row.time)}>
            Use {timecode(row.time - report.window.start)}
          </button>
        {/if}
      </li>
    {/each}
  </ul>
{/if}
