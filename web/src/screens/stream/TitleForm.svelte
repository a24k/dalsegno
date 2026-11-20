<script lang="ts">
  import { api } from '../../lib/api'
  import type { Feedback } from '../../lib/feedback.svelte'
  import type { Stream } from '../../lib/types'

  let {
    stream,
    feedback,
    changed,
  }: { stream: Stream; feedback: Feedback; changed: (stream: Stream) => void } = $props()

  let title = $derived(stream.title)

  function submit(event: SubmitEvent): void {
    event.preventDefault()
    void feedback.run(
      async () =>
        changed(await api.patch<Stream>(`/streams/${stream.id}`, { title: title.trim() })),
      'The title is saved.',
      'The title could not be saved.',
    )
  }
</script>

<form onsubmit={submit}>
  <div class="field">
    <label for="stream-title">Title</label>
    <input id="stream-title" type="text" required maxlength="300" bind:value={title} />
  </div>
  <button type="submit">Save title</button>
</form>
