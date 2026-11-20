<script lang="ts">
  import { percent } from '../../lib/format'
  import type { Job } from '../../lib/types'

  let {
    job,
    stalled,
    problem,
    retry,
  }: { job: Job | null; stalled: Job | null; problem: string | null; retry: () => void } = $props()
</script>

{#if problem !== null}
  <p role="alert">
    Finding boundary candidates failed: {problem}
    <button type="button" onclick={retry}>Try again</button>
  </p>
{:else if stalled !== null}
  <p role="alert">
    The candidates are still not ready after a finished job. Try again, or check the job on the Jobs
    screen.
    <a href="/jobs">Job {stalled.id}</a>
    <button type="button" onclick={retry}>Try again</button>
  </p>
{:else if job !== null}
  <p role="status">
    Finding boundary candidates: job {job.id} is {job.state}
    {#if job.progress !== null}
      <progress max="1" value={job.progress} aria-label="Job {job.id} progress"></progress>
      {percent(job.progress)}
    {/if}
  </p>
{/if}
