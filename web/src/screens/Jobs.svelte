<script lang="ts">
  import { api, ApiError } from '../lib/api'
  import { ago, percent } from '../lib/format'
  import ListState from '../lib/ListState.svelte'
  import { Clock, LiveList } from '../lib/live-list.svelte'
  import { isActive, type Job } from '../lib/types'

  const PAGE = 50

  const jobs = new LiveList<Job>('/jobs', 'job.progress', 'newest-first', PAGE)
  const clock = new Clock()
  let problem = $state<string | null>(null)

  $effect(() => jobs.start())
  $effect(() => clock.start())

  function about(job: Job): string {
    const stream = job.target?.stream
    return typeof stream === 'string' ? stream : ''
  }

  async function cancel(job: Job): Promise<void> {
    problem = null
    try {
      await api.post(`/jobs/${job.id}/cancel`)
      // The live update shows the new state; this covers a page that is not connected.
      await jobs.load()
    } catch (error) {
      problem = error instanceof ApiError ? error.message : 'The job could not be cancelled.'
    }
  }
</script>

<h1>Jobs</h1>

{#if problem}
  <p role="alert">{problem}</p>
{/if}

<ListState list={jobs} empty="No job has run yet.">
  {#snippet children(items)}
    <table>
      <thead>
        <tr>
          <th scope="col">Job</th>
          <th scope="col">About</th>
          <th scope="col">State</th>
          <th scope="col">Progress</th>
          <th scope="col">When</th>
          <th scope="col"><span class="visually-hidden">Actions</span></th>
        </tr>
      </thead>
      <tbody>
        {#each items as job (job.id)}
          <tr>
            <th scope="row">{job.id} {job.kind}</th>
            <td>{about(job)}</td>
            <td>
              <span class="state" data-state={job.state}>{job.state}</span>
              {#if job.state === 'failed' && job.error}
                <p class="why">Why: {job.error}</p>
              {/if}
            </td>
            <td>
              {#if job.progress !== null}
                <progress max="1" value={job.progress} aria-label="Job {job.id} progress"
                ></progress>
                {percent(job.progress)}
              {/if}
            </td>
            <td>
              queued {ago(job.created_at, clock.now)}
              {#if job.finished_at !== null}, finished {ago(job.finished_at, clock.now)}{/if}
            </td>
            <td>
              {#if isActive(job)}
                <button type="button" onclick={() => cancel(job)}>Cancel job {job.id}</button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if jobs.total !== null && jobs.total > items.length}
      <p class="muted">The newest {items.length} of {jobs.total} jobs.</p>
    {/if}
  {/snippet}
</ListState>
