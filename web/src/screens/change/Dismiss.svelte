<script lang="ts">
  import { api } from '../../lib/api'
  import type { Feedback } from '../../lib/feedback.svelte'
  import type { ChangeDetail } from '../../lib/types'

  let {
    changeId,
    feedback,
    dismissed,
  }: { changeId: number; feedback: Feedback; dismissed: (change: ChangeDetail) => void } = $props()

  let isAsking = $state(false)

  function dismiss(): void {
    isAsking = false
    void feedback.run(
      async () => {
        dismissed(await api.post<ChangeDetail>(`/changes/${changeId}/dismiss`))
      },
      'The change is dismissed.',
      'The change could not be dismissed.',
    )
  }
</script>

<section aria-labelledby="dismiss-heading">
  <h2 id="dismiss-heading">Dismiss</h2>
  {#if isAsking}
    <p>Dismiss this change? Its holds lapse after 7 days.</p>
    <button type="button" onclick={dismiss}>Yes, dismiss</button>
    <button type="button" onclick={() => (isAsking = false)}>Keep the change</button>
  {:else}
    <button type="button" onclick={() => (isAsking = true)}>Dismiss change</button>
  {/if}
</section>
