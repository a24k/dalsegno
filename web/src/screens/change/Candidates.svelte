<script lang="ts">
  import { api } from '../../lib/api'
  import type { Feedback } from '../../lib/feedback.svelte'
  import { clock } from '../../lib/format'
  import { clipBody, type Candidate } from '../../lib/selection'

  let {
    changeId,
    candidates,
    feedback,
    made,
  }: { changeId: number; candidates: Candidate[]; feedback: Feedback; made: () => void } = $props()

  function make(candidate: Candidate): void {
    const body = clipBody(changeId, candidate.kind, candidate.versionId, candidate)
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

<section aria-labelledby="candidates-heading">
  <h2 id="candidates-heading">Suggested clips</h2>
  {#if candidates.length === 0}
    <p>No clip is suggested for this change.</p>
  {:else}
    <ul>
      {#each candidates as candidate (candidate.label)}
        <li>
          {candidate.label}, {candidate.kind}, {clock(candidate.end - candidate.start)}
          <button type="button" onclick={() => make(candidate)}>
            Make clip: {candidate.label}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</section>
