<script lang="ts">
  import { ago, holes, percent, period, timeLeft } from '../lib/format'
  import ListState from '../lib/ListState.svelte'
  import { Clock, LiveList } from '../lib/live-list.svelte'
  import { isOpen, type Change, type Job, type Stream } from '../lib/types'

  const LIST_LIMIT = 200
  const RECENT_JOBS = 10

  const streams = new LiveList<Stream>('/streams', 'stream.state', 'oldest-first', LIST_LIMIT)
  const changes = new LiveList<Change>('/changes', 'change.updated', 'newest-first', LIST_LIMIT)
  const jobs = new LiveList<Job>('/jobs', 'job.progress', 'newest-first', RECENT_JOBS)
  const clock = new Clock()

  $effect(() => streams.start())
  $effect(() => changes.start())
  $effect(() => jobs.start())
  $effect(() => clock.start())

  let titles = $derived(
    Object.fromEntries(streams.items.map((stream) => [stream.id, stream.title])),
  )
  let openChanges = $derived(changes.items.filter(isOpen))
</script>

<h1>Dashboard</h1>

<section aria-labelledby="streams-heading">
  <h2 id="streams-heading">Streams</h2>
  <ListState list={streams} empty="No streams yet. Add one with dalsegno add.">
    {#snippet children(items)}
      <table>
        <thead>
          <tr>
            <th scope="col">Stream</th>
            <th scope="col">State</th>
            <th scope="col">Period</th>
            <th scope="col">Holes</th>
            <th scope="col">Last fetch</th>
          </tr>
        </thead>
        <tbody>
          {#each items as stream (stream.id)}
            <tr>
              <th scope="row"><a href="/streams/{stream.id}">{stream.title}</a></th>
              <td>
                <span class="state" data-state={stream.state}>{stream.state}</span>
                {#if stream.state_reason}({stream.state_reason}){/if}
              </td>
              <td>{period(stream.period)}</td>
              <td>{holes(stream.open_fragments, stream.lost_fragments)}</td>
              <td>{ago(stream.last_fetch_at, clock.now)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/snippet}
  </ListState>
</section>

<section aria-labelledby="changes-heading">
  <h2 id="changes-heading">Open changes</h2>
  <ListState list={changes} items={openChanges} empty="No change is waiting.">
    {#snippet children(items)}
      <table>
        <thead>
          <tr>
            <th scope="col">Change</th>
            <th scope="col">Stream</th>
            <th scope="col">State</th>
            <th scope="col">Seen</th>
            <th scope="col">DVR</th>
          </tr>
        </thead>
        <tbody>
          {#each items as change (change.id)}
            <tr>
              <th scope="row"><a href="/changes/{change.id}">Change {change.id}</a></th>
              <td>{titles[change.stream_id] ?? `Stream ${change.stream_id}`}</td>
              <td><span class="state" data-state={change.state}>{change.state}</span></td>
              <td>{ago(change.detected_at, clock.now)}</td>
              <td>{timeLeft(change.dvr_deadline_at, clock.now)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/snippet}
  </ListState>
</section>

<section aria-labelledby="jobs-heading">
  <h2 id="jobs-heading">Recent jobs</h2>
  <ListState list={jobs} empty="No job has run yet.">
    {#snippet children(items)}
      <ul class="jobs">
        {#each items as job (job.id)}
          <li>
            <span>Job {job.id}, {job.kind}:</span>
            <span class="state" data-state={job.state}>{job.state}</span>
            {#if job.state === 'running' && job.progress !== null}
              <progress max="1" value={job.progress} aria-label="Job {job.id} progress"></progress>
              {percent(job.progress)}
            {/if}
            <span class="muted">
              {ago(job.finished_at ?? job.started_at ?? job.created_at, clock.now)}
            </span>
          </li>
        {/each}
      </ul>
      <p><a href="/jobs">All jobs</a></p>
    {/snippet}
  </ListState>
</section>
