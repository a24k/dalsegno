<script lang="ts">
  import type { MeasuredPeriod } from '../../lib/types'

  let { period }: { period: MeasuredPeriod | null } = $props()
</script>

<p>
  Period:
  {#if period === null}
    not measured
  {:else}
    audio {period.audio.toFixed(1)} s,
    {#if period.video === null}
      video not found
    {:else}
      video {period.video.period.toFixed(2)} s (from {period.video.agreeing} agreeing edges)
    {/if}
  {/if}
</p>
