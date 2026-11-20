<script lang="ts">
  import { api } from './api'
  import type { Feedback } from './feedback.svelte'
  import { canStart, canStop, type Stream } from './types'

  let {
    stream,
    feedback,
    changed,
  }: { stream: Stream; feedback: Feedback; changed: (stream: Stream) => void } = $props()

  async function act(action: 'start' | 'stop'): Promise<void> {
    const done = action === 'start' ? 'started' : 'stopped'
    await feedback.run(
      async () => changed(await api.post<Stream>(`/streams/${stream.id}/${action}`)),
      `${stream.title} ${done}.`,
      `The stream could not be ${done}.`,
    )
  }
</script>

{#if canStart(stream)}
  <button type="button" onclick={() => act('start')}>Start {stream.title}</button>
{/if}
{#if canStop(stream)}
  <button type="button" onclick={() => act('stop')}>Stop {stream.title}</button>
{/if}
