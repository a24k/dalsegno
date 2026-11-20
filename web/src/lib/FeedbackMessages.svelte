<script lang="ts">
  import type { Feedback } from './feedback.svelte'

  let { feedback }: { feedback: Feedback } = $props()
</script>

<div role="status">
  {#if feedback.notice}<p>{feedback.notice}</p>{/if}
</div>
{#if feedback.problem}
  <p role="alert">{feedback.problem}</p>
{/if}
