<script lang="ts">
  import { api } from '../../lib/api'
  import type { Feedback } from '../../lib/feedback.svelte'
  import { knownClipBody } from '../../lib/forms'
  import type { KnownClip, Program } from '../../lib/types'
  import type { StreamDetail } from './detail.svelte'

  let {
    detail,
    program,
    clips,
    feedback,
  }: { detail: StreamDetail; program: Program; clips: KnownClip[]; feedback: Feedback } = $props()

  let name = $state('')
  let from = $state('')
  let to = $state('')

  function add(event: SubmitEvent): void {
    event.preventDefault()
    const built = knownClipBody(detail.id, name, from, to)
    if ('problem' in built) {
      feedback.notice = null
      feedback.problem = built.problem
      return
    }
    void feedback.run(
      async () => {
        await api.post(`/programs/${program.id}/known-clips`, built.body)
        name = ''
        from = ''
        to = ''
        await detail.loadProgram()
      },
      'The known clip is saved.',
      'The known clip could not be saved.',
    )
  }

  function remove(clip: KnownClip): void {
    void feedback.run(
      async () => {
        await api.delete(`/programs/${program.id}/known-clips/${clip.id}`)
        await detail.loadProgram()
      },
      `${clip.name} is deleted.`,
      'The known clip could not be deleted.',
    )
  }
</script>

<section aria-labelledby="known-heading">
  <h2 id="known-heading">Known material</h2>
  <p class="muted">
    Stretches of this program that are known, such as an opening. They are stored now, and the
    analysis will set them aside later.
  </p>
  {#if clips.length === 0}
    <p>No known clip yet.</p>
  {:else}
    <ul>
      {#each clips as clip (clip.id)}
        <li>
          {clip.name}, {clip.duration.toFixed(1)} s
          <button type="button" onclick={() => remove(clip)}>Delete {clip.name}</button>
        </li>
      {/each}
    </ul>
  {/if}
  <form onsubmit={add}>
    <div class="field">
      <label for="known-name">Name</label>
      <input id="known-name" type="text" required maxlength="200" bind:value={name} />
    </div>
    <div class="field">
      <label for="known-from">From, in seconds of stream time</label>
      <input id="known-from" type="number" min="0" step="any" required bind:value={from} />
    </div>
    <div class="field">
      <label for="known-to">To, in seconds of stream time</label>
      <input id="known-to" type="number" min="0" step="any" required bind:value={to} />
    </div>
    <button type="submit">Add the known clip</button>
  </form>
</section>
