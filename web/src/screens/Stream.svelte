<script lang="ts">
  import { Feedback } from '../lib/feedback.svelte'
  import FeedbackMessages from '../lib/FeedbackMessages.svelte'
  import { Clock } from '../lib/live-list.svelte'
  import { router } from '../lib/router.svelte'
  import StreamControls from '../lib/StreamControls.svelte'
  import { StreamDetail } from './stream/detail.svelte'
  import History from './stream/History.svelte'
  import KnownMaterial from './stream/KnownMaterial.svelte'
  import ProgramSection from './stream/ProgramSection.svelte'
  import TitleForm from './stream/TitleForm.svelte'

  const detail = new StreamDetail(Number(router.route.params.id))
  const feedback = new Feedback()
  const clock = new Clock()

  $effect(() => detail.start())
  $effect(() => clock.start())
</script>

{#if detail.isMissing || Number.isNaN(detail.id)}
  <h1>Not found</h1>
  <p>There is no such stream. <a href="/streams">All streams</a></p>
{:else if detail.stream}
  {@const stream = detail.stream}
  <h1>{stream.title}</h1>
  <FeedbackMessages {feedback} />
  {#if detail.error}
    <p role="alert">{detail.error}</p>
  {/if}
  <section aria-labelledby="state-heading">
    <h2 id="state-heading">State</h2>
    <p>
      <span class="state" data-state={stream.state}>{stream.state}</span>
      {#if stream.state_reason}({stream.state_reason}){/if}
      <span class="muted">Video ID {stream.video_id}</span>
    </p>
    <StreamControls {stream} {feedback} changed={(next) => void detail.show(next)} />
  </section>
  <section aria-labelledby="title-heading">
    <h2 id="title-heading">Title</h2>
    <TitleForm {stream} {feedback} changed={(next) => void detail.show(next)} />
  </section>
  <ProgramSection {detail} {stream} {feedback} />
  {#if detail.program}
    <History versions={detail.versions} changes={detail.changes} now={clock.now} />
    <KnownMaterial {detail} program={detail.program} clips={detail.clips} {feedback} />
  {/if}
{:else if detail.error}
  <p role="alert">{detail.error}</p>
{:else}
  <p>Loading…</p>
{/if}
