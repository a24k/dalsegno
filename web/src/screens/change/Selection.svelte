<script lang="ts">
  import { api } from '../../lib/api'
  import type { Feedback } from '../../lib/feedback.svelte'
  import { clock } from '../../lib/format'
  import { clipBody, defaultKind, selectionSpan, type Selection } from '../../lib/selection'
  import type { ClipKind, Comparison } from '../../lib/types'

  let {
    changeId,
    comparison,
    selection,
    feedback,
    made,
  }: {
    changeId: number
    comparison: Comparison
    selection: Selection | null
    feedback: Feedback
    made: () => void
  } = $props()

  let bar = $derived(selection === null ? null : comparison[selection.bar])
  let span = $derived(bar && selection ? selectionSpan(bar, selection) : null)
  // The kind follows the bar until the operator picks one, and then again on the next choice.
  let chosen = $state<{ bar: string; kind: ClipKind } | null>(null)
  let kind = $derived(
    selection && chosen?.bar === selection.bar
      ? chosen.kind
      : selection
        ? defaultKind(selection.bar)
        : 'addition',
  )

  function make(event: SubmitEvent): void {
    event.preventDefault()
    if (!bar || !span) {
      return
    }
    const body = clipBody(changeId, kind, bar.version_id, span)
    void feedback.run(
      async () => {
        await api.post('/clips', body)
        made()
      },
      'The clip is made.',
      'The clip could not be made.',
    )
  }
</script>

<section aria-labelledby="selection-heading">
  <h2 id="selection-heading">Selected regions</h2>
  {#if span && selection}
    <form onsubmit={make}>
      <dl>
        <dt>Start</dt>
        <dd>{clock(span.start - (bar?.start ?? 0))} (stream time {span.start.toFixed(1)} s)</dd>
        <dt>End</dt>
        <dd>{clock(span.end - (bar?.start ?? 0))} (stream time {span.end.toFixed(1)} s)</dd>
        <dt>Length</dt>
        <dd>{clock(span.end - span.start)} ({(span.end - span.start).toFixed(1)} s)</dd>
      </dl>
      <div class="field">
        <label for="selection-kind">Kind</label>
        <select
          id="selection-kind"
          value={kind}
          onchange={(event) => {
            chosen = {
              bar: selection.bar,
              kind: event.currentTarget.value === 'cycle' ? 'cycle' : 'addition',
            }
          }}
        >
          <option value="addition">addition</option>
          <option value="cycle">cycle</option>
        </select>
      </div>
      <button type="submit">Make clip</button>
    </form>
  {:else}
    <p class="muted">Choose regions on a bar above, or make one of the suggested clips below.</p>
  {/if}
</section>
