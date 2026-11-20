<script lang="ts">
  import { Feedback } from '../lib/feedback.svelte'
  import FeedbackMessages from '../lib/FeedbackMessages.svelte'
  import { router } from '../lib/router.svelte'
  import { ClipEditor } from './clip/editor.svelte'
  import EdgePanel from './clip/EdgePanel.svelte'
  import Finding from './clip/Finding.svelte'
  import Header from './clip/Header.svelte'
  import Period from './clip/Period.svelte'

  const editor = new ClipEditor(Number(router.route.params.id))
  const feedback = new Feedback()

  $effect(() => editor.start())

  function save(): void {
    void feedback.run(() => editor.save(), 'Saved.', 'The boundaries could not be saved.')
  }
</script>

{#if editor.isMissing || Number.isNaN(editor.id)}
  <h1>Not found</h1>
  <p>There is no such clip. <a href="/">Dashboard</a></p>
{:else if editor.candidates && editor.clip}
  {@const clip = editor.clip}
  {@const readOnly = clip.state === 'exported'}
  <h1>Edit boundaries of clip {clip.id}</h1>
  <FeedbackMessages {feedback} />
  {#if editor.error}
    <p role="alert">{editor.error}</p>
  {/if}
  <Header {clip} />
  <Finding
    job={editor.finding}
    stalled={editor.stalledJob}
    problem={editor.jobProblem}
    retry={() => void editor.retry()}
  />
  <p class="muted">
    Keys inside a panel: left and right arrows move the playhead 0.1 s, with Shift 1 s; comma and
    full stop jump to the previous and next keyframe; Enter on the seek bar sets the edge to the
    playhead; Space plays and pauses.
  </p>
  <EdgePanel
    clipId={clip.id}
    edge="start"
    report={editor.candidates.start}
    saved={clip.start}
    draft={editor.drafts.start}
    {readOnly}
    edit={(time) => editor.edit('start', time)}
  />
  <EdgePanel
    clipId={clip.id}
    edge="end"
    report={editor.candidates.end}
    saved={clip.end}
    draft={editor.drafts.end}
    {readOnly}
    edit={(time) => editor.edit('end', time)}
  />
  {#if clip.kind === 'cycle'}
    <Period period={editor.candidates.period} />
  {/if}
  {#if !readOnly}
    <button type="button" disabled={editor.unsaved.length === 0} onclick={save}>Save</button>
    {#if editor.unsaved.length > 0}
      <span class="state" data-state="unsaved">unsaved changes</span>
    {/if}
  {/if}
{:else if editor.error}
  <p role="alert">{editor.error}</p>
{:else}
  <p>Loading…</p>
{/if}
