<script lang="ts">
  import { placement, span, timeAtRatio } from '../../lib/editor'
  import type { EdgeReport } from '../../lib/types'

  let {
    report,
    edgeTime,
    playhead,
    label,
    seek,
    set,
  }: {
    report: EdgeReport
    edgeTime: number | null
    /** The stream time of the video's position. */
    playhead: number
    label: string
    seek: (time: number) => void
    set: () => void
  } = $props()

  let track = $state<HTMLElement | null>(null)

  function click(event: MouseEvent): void {
    if (!track) {
      return
    }
    const box = track.getBoundingClientRect()
    if (box.width > 0) {
      seek(timeAtRatio(report.window, (event.clientX - box.left) / box.width))
    }
  }

  function key(event: KeyboardEvent): void {
    if (event.key === 'Enter') {
      event.preventDefault()
      set()
    }
  }

  let edgeAt = $derived(edgeTime === null ? null : placement(report.window, edgeTime))
  let playheadAt = $derived(placement(report.window, playhead))
</script>

<div
  class="seek"
  bind:this={track}
  role="slider"
  tabindex="0"
  aria-label={label}
  aria-valuemin={report.window.start}
  aria-valuemax={report.window.end}
  aria-valuenow={playhead}
  aria-valuetext="{playhead.toFixed(1)} s"
  onclick={click}
  onkeydown={key}
>
  {#each report.black as interval, index (index)}
    {@const box = span(report.window, interval)}
    {#if box}
      <span class="black" style="left: {box.left}%; width: {box.width}%"></span>
    {/if}
  {/each}
  {#each report.silence as interval, index (index)}
    {@const box = span(report.window, interval)}
    {#if box}
      <span class="silence" style="left: {box.left}%; width: {box.width}%"></span>
    {/if}
  {/each}
  {#each report.keyframes as keyframe (keyframe.sq)}
    {@const at = placement(report.window, keyframe.time)}
    {#if at !== null}
      <span class="tick" style="left: {at}%"></span>
    {/if}
  {/each}
  {#if edgeAt !== null}
    <span class="edge" style="left: {edgeAt}%"></span>
  {/if}
  {#if playheadAt !== null}
    <span class="playhead" style="left: {playheadAt}%"></span>
  {/if}
</div>
<ul class="legend muted">
  <li><span class="key key-black"></span> black</li>
  <li><span class="key key-silence"></span> silence</li>
  <li><span class="key key-tick"></span> keyframe</li>
  <li><span class="key key-edge"></span> clip edge</li>
  <li><span class="key key-playhead"></span> playhead</li>
</ul>

<style>
  .seek {
    position: relative;
    height: 3rem;
    margin: 0.5rem 0;
    border: 1px solid currentColor;
    cursor: pointer;
  }

  .seek > span {
    position: absolute;
  }

  .black {
    top: 0;
    height: 40%;
    background: repeating-linear-gradient(45deg, currentColor 0 3px, transparent 3px 6px);
  }

  .silence {
    top: 40%;
    height: 30%;
    background: repeating-linear-gradient(90deg, currentColor 0 2px, transparent 2px 5px);
    opacity: 0.7;
  }

  .tick {
    bottom: 0;
    width: 1px;
    height: 30%;
    background: currentColor;
  }

  .edge {
    top: 0;
    bottom: 0;
    width: 0;
    border-left: 3px double currentColor;
  }

  .playhead {
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--alert);
  }

  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 1rem;
    padding: 0;
    margin: 0;
    list-style: none;
    font-size: 0.875rem;
  }

  .key {
    position: relative;
    width: 1rem;
    height: 0.75rem;
    vertical-align: middle;
    border: 1px solid currentColor;
  }

  .key-black {
    background: repeating-linear-gradient(45deg, currentColor 0 3px, transparent 3px 6px);
  }

  .key-silence {
    background: repeating-linear-gradient(90deg, currentColor 0 2px, transparent 2px 5px);
  }

  .key-tick,
  .key-edge,
  .key-playhead {
    width: 0;
    border-width: 0 0 0 1px;
  }

  .key-edge {
    border-left: 3px double currentColor;
  }

  .key-playhead {
    border-left: 2px solid var(--alert);
  }
</style>
