<script lang="ts">
  import { clock } from '../../lib/format'
  import type { Clip } from '../../lib/types'

  let { clip }: { clip: Clip } = $props()
</script>

<p>
  <span class="state" data-state={clip.state}>{clip.state}</span>
  {clip.kind}
  {#if clip.start !== null && clip.end !== null}
    {clip.start.toFixed(1)} s to {clip.end.toFixed(1)} s, {clock(clip.end - clip.start)}
  {:else}
    <span class="muted">No span</span>
  {/if}
  {#if clip.change_id !== null}
    <a href="/changes/{clip.change_id}">Back to change {clip.change_id}</a>
  {/if}
</p>
{#if clip.state === 'exported'}
  <p>This clip is exported, so its boundaries cannot be changed.</p>
{/if}
