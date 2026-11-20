<script lang="ts">
  import { api } from '../../lib/api'
  import { Feedback } from '../../lib/feedback.svelte'
  import FeedbackMessages from '../../lib/FeedbackMessages.svelte'
  import type { AddedStream } from '../../lib/types'

  let { added }: { added: (stream: AddedStream['stream']) => void } = $props()

  const feedback = new Feedback()
  let source = $state('')
  let isProbing = $state(false)
  let latest = $state<AddedStream['stream'] | null>(null)

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault()
    isProbing = true
    latest = null
    await feedback.run(
      async () => {
        const result = await api.post<AddedStream>('/streams', { stream: source.trim() })
        latest = result.stream
        source = ''
        added(result.stream)
      },
      'The stream is added.',
      'The stream could not be added.',
    )
    isProbing = false
  }
</script>

<section aria-labelledby="add-heading">
  <h2 id="add-heading">Add a stream</h2>
  <form onsubmit={submit}>
    <div class="field">
      <label for="add-source">Video ID or URL</label>
      <input id="add-source" type="text" required bind:value={source} />
    </div>
    <button type="submit" disabled={isProbing}>{isProbing ? 'Probing…' : 'Add the stream'}</button>
  </form>
  {#if isProbing}
    <p>Probing the stream. This can take a few minutes.</p>
  {/if}
  <FeedbackMessages {feedback} />
  {#if latest}
    <p><a href="/streams/{latest.id}">Open {latest.title}</a></p>
  {/if}
</section>
