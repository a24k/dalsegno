<script lang="ts">
  import {
    fromPlayhead,
    nextKeyframe,
    playheadStep,
    previousKeyframe,
    snapToKeyframe,
    timecode,
    toStreamTime,
    toVideoTime,
  } from '../../lib/editor'
  import type { Edge, EdgeReport } from '../../lib/types'
  import EdgeCandidates from './EdgeCandidates.svelte'
  import SeekBar from './SeekBar.svelte'

  let {
    clipId,
    edge,
    report,
    saved,
    draft,
    readOnly,
    edit,
  }: {
    clipId: number
    edge: Edge
    report: EdgeReport | null
    saved: number | null
    draft: number | null
    readOnly: boolean
    edit: (time: number | null) => void
  } = $props()

  const heading = $derived(edge === 'start' ? 'Start' : 'End')
  let video = $state<HTMLVideoElement | null>(null)
  let videoTime = $state(0)

  let entered = $derived(draft ?? saved)
  let isUnsaved = $derived(draft !== null && draft !== saved)
  let playhead = $derived(report ? toStreamTime(report, videoTime) : 0)
  let snapped = $derived(
    edge === 'start' && report && entered !== null
      ? snapToKeyframe(report.keyframes, entered)
      : null,
  )
  let version = $derived(report ? encodeURIComponent(String(report.around)) : '')

  function seek(time: number): void {
    if (video && report) {
      video.currentTime = Math.max(0, toVideoTime(report, time))
    }
  }

  function setToPlayhead(): void {
    if (report && !readOnly) {
      edit(fromPlayhead(edge, report.keyframes, playhead))
    }
  }

  function typed(event: Event): void {
    const input = event.currentTarget as HTMLInputElement
    edit(Number.isFinite(input.valueAsNumber) ? input.valueAsNumber : null)
  }

  function key(event: KeyboardEvent): void {
    if (!report || event.target instanceof HTMLInputElement) {
      return
    }
    const step = playheadStep(event.key, event.shiftKey)
    const jump =
      event.key === ','
        ? previousKeyframe(report.keyframes, playhead)
        : event.key === '.'
          ? nextKeyframe(report.keyframes, playhead)
          : null
    if (step !== null) {
      seek(playhead + step)
    } else if (jump) {
      seek(jump.time)
    } else if (event.key !== ',' && event.key !== '.') {
      return
    }
    event.preventDefault()
  }
</script>

<!-- The keys act only with focus inside the panel, as the help text says. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="panel" aria-labelledby="{edge}-heading" onkeydown={key}>
  <h3 id="{edge}-heading">{heading}</h3>
  {#if report === null}
    <p>No preview for this edge yet.</p>
  {:else}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video
      bind:this={video}
      bind:currentTime={videoTime}
      controls
      preload="metadata"
      playsinline
      src="/api/v1/clips/{clipId}/preview?edge={edge}&v={version}"
    ></video>
    <SeekBar
      {report}
      edgeTime={saved}
      {playhead}
      label="Seek the {edge} preview"
      {seek}
      set={setToPlayhead}
    />
    <p class="muted">
      Window {timecode(0)} to {timecode(report.window.end - report.window.start)} from stream time
      {report.window.start.toFixed(1)} s. Playhead {timecode(playhead - report.window.start)}.
    </p>
  {/if}
  <p>
    Saved {edge}: {saved === null ? 'none' : `${saved.toFixed(3)} s`}
    {#if isUnsaved}
      <span class="state" data-state="unsaved">unsaved change</span>
    {/if}
  </p>
  {#if !readOnly}
    <label>
      {heading} in seconds of stream time
      <input type="number" step="0.001" value={entered} oninput={typed} />
    </label>
    {#if report && entered !== null}
      <span class="muted">{timecode(entered - report.window.start)} from the window start</span>
    {/if}
    {#if edge === 'start' && entered !== null && report}
      <p>
        {#if snapped}
          Starts at the keyframe at {snapped.time.toFixed(3)} s (sq {snapped.sq}).
        {:else}
          No keyframe at or before this time is in the window; the service will find one.
        {/if}
      </p>
    {/if}
    {#if report}
      <button type="button" onclick={setToPlayhead}>Set {edge} to playhead</button>
    {/if}
  {/if}
  {#if report}
    <EdgeCandidates {edge} {report} {readOnly} use={(time) => edit(time)} />
    <img
      alt="Contact sheet around the {edge}"
      src="/api/v1/clips/{clipId}/sheet?edge={edge}&v={version}"
    />
  {/if}
</section>

<style>
  .panel {
    margin: 1rem 0;
  }

  video,
  img {
    max-width: 100%;
  }
</style>
