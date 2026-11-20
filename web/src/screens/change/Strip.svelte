<script lang="ts">
  import { isSelected, type BarName, type Selection } from '../../lib/selection'
  import {
    axisLength,
    boundaryLabel,
    isWide,
    place,
    placePoint,
    regionLabel,
    regionName,
    ticks,
  } from '../../lib/strip'
  import type { Bar, Comparison } from '../../lib/types'
  import './strip.css'

  let {
    comparison,
    selection,
    toggle,
  }: {
    comparison: Comparison
    selection: Selection | null
    toggle: (bar: BarName, index: number) => void
  } = $props()

  let axis = $derived(axisLength(comparison))
  let marks = $derived(ticks(axis))
</script>

{#snippet barRow(name: BarName, title: string, bar: Bar)}
  <h3 id="{name}-bar">{title}</h3>
  <div class="track" role="group" aria-labelledby="{name}-bar">
    {#each bar.regions as region, index (region.id)}
      {@const placement = place(bar, region, axis)}
      <button
        type="button"
        class="block"
        data-class={region.class}
        style="left: {placement.left}%; width: {placement.width}%"
        title={regionName(bar, region)}
        aria-label={regionName(bar, region)}
        aria-pressed={isSelected(selection, name, index)}
        onclick={() => toggle(name, index)}
      >
        {#if isWide(placement)}{regionLabel(region)}{/if}
      </button>
    {/each}
    {#each bar.regions as region (region.id)}
      {#each region.video_boundaries as boundary (boundary)}
        <span
          class="boundary"
          role="img"
          aria-label={boundaryLabel(bar, boundary)}
          title={boundaryLabel(bar, boundary)}
          style="left: {placePoint(bar, boundary, axis)}%"
        ></span>
      {/each}
    {/each}
  </div>
{/snippet}

<section class="strip" aria-labelledby="strip-heading">
  <h2 id="strip-heading">Versions</h2>
  <p class="muted">
    Both bars start at their own cycle start, on one time axis. Choose neighbouring regions to make
    a clip.
  </p>
  <div class="axis" aria-hidden="true">
    {#each marks as tick (tick.left)}
      <span class="tick" style="left: {tick.left}%" data-label={tick.label ?? undefined}>
        {#if tick.label}<span>{tick.label}</span>{/if}
      </span>
    {/each}
  </div>
  {#if comparison.old}
    {@render barRow('old', 'Old version', comparison.old)}
  {/if}
  {#if comparison.new}
    {@render barRow('new', 'New version', comparison.new)}
  {:else}
    <p>No new version yet: the change is still provisional.</p>
  {/if}
</section>
