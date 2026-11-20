<script lang="ts">
  import { api, ApiError } from '../lib/api'
  import { inJapan } from '../lib/format'
  import type { Stream } from '../lib/types'

  interface Setting {
    value: number | string
    default: number | string
    overridden: boolean
  }

  interface SettingsView {
    operational: Record<string, Setting>
    bootstrap: Record<string, string | boolean | null>
  }

  interface Doctor {
    doctor: {
      tools: { name: string; path: string | null; version: string | null }[]
      has_ipv6_route: boolean
      limits: { concurrency: number; requests_per_second: number }
      policy: string
      active_family: string
      last_switch: { at: number; from: string; to: string; reason: string } | null
      resolution: { family: string; resolved: boolean; error: string | null }[]
    }
    probe: Record<string, unknown> | null
  }

  const LABELS: Record<string, string> = {
    concurrency: 'Requests in flight',
    requests_per_second: 'Request starts per second',
    daily_summary: 'Daily summary, Japan time',
  }
  const OFF = 'off'

  let view = $state<SettingsView | null>(null)
  let streams = $state<Stream[]>([])
  let concurrency = $state('')
  let rate = $state('')
  let isSummaryOn = $state(false)
  let summaryAt = $state('09:00')
  let notice = $state<string | null>(null)
  let problem = $state<string | null>(null)
  let doctorStream = $state('')
  let doctor = $state<Doctor | null>(null)
  let isRunning = $state(false)
  let doctorProblem = $state<string | null>(null)

  function message(error: unknown, fallback: string): string {
    return error instanceof ApiError ? error.message : fallback
  }

  function show(next: SettingsView): void {
    view = next
    concurrency = String(next.operational.concurrency.value)
    rate = String(next.operational.requests_per_second.value)
    const summary = String(next.operational.daily_summary.value)
    isSummaryOn = summary !== OFF
    summaryAt = isSummaryOn ? summary : summaryAt
  }

  async function load(): Promise<void> {
    try {
      show((await api.get<SettingsView>('/settings')).items)
      streams = (await api.get<Stream[]>('/streams?limit=200')).items
    } catch (error) {
      problem = message(error, 'The settings could not be read.')
    }
  }

  $effect(() => {
    void load()
  })

  async function save(change: Record<string, unknown>, done: string): Promise<void> {
    notice = null
    problem = null
    try {
      show(await api.put<SettingsView>('/settings', change))
      notice = done
    } catch (error) {
      problem = message(error, 'The settings could not be saved.')
    }
  }

  function submit(event: SubmitEvent): void {
    event.preventDefault()
    void save(
      {
        concurrency: Number(concurrency),
        requests_per_second: Number(rate),
        daily_summary: isSummaryOn ? summaryAt : OFF,
      },
      'Saved. The new values apply at once.',
    )
  }

  function useDefault(key: string): void {
    void save({ [key]: null }, `${LABELS[key]} is back to its default.`)
  }

  async function runDoctor(event: SubmitEvent): Promise<void> {
    event.preventDefault()
    isRunning = true
    doctorProblem = null
    try {
      const query = doctorStream === '' ? '' : `?stream=${doctorStream}`
      doctor = (await api.get<Doctor>(`/doctor${query}`)).items
    } catch (error) {
      doctorProblem = message(error, 'The diagnostics could not run.')
    } finally {
      isRunning = false
    }
  }

  function bootstrapValue(value: string | boolean | null): string {
    if (value === true) return 'set'
    if (value === false) return 'not set'
    return value ?? 'not set'
  }
</script>

<h1>Settings and diagnostics</h1>

<div role="status">
  {#if notice}<p>{notice}</p>{/if}
</div>
{#if problem}
  <p role="alert">{problem}</p>
{/if}

<section aria-labelledby="operational-heading">
  <h2 id="operational-heading">Operational settings</h2>
  {#if view}
    <form onsubmit={submit}>
      <div class="field">
        <label for="concurrency">{LABELS.concurrency}</label>
        <input
          id="concurrency"
          type="number"
          min="1"
          max="64"
          step="1"
          required
          bind:value={concurrency}
        />
        <p class="muted">Default {view.operational.concurrency.default}.</p>
        {#if view.operational.concurrency.overridden}
          <button type="button" onclick={() => useDefault('concurrency')}>
            Use the default for requests in flight
          </button>
        {/if}
      </div>
      <div class="field">
        <label for="rate">{LABELS.requests_per_second}</label>
        <input id="rate" type="number" min="0.01" step="any" required bind:value={rate} />
        <p class="muted">Default {view.operational.requests_per_second.default}.</p>
        {#if view.operational.requests_per_second.overridden}
          <button type="button" onclick={() => useDefault('requests_per_second')}>
            Use the default for request starts
          </button>
        {/if}
      </div>
      <fieldset class="field">
        <legend>{LABELS.daily_summary}</legend>
        <label><input type="checkbox" bind:checked={isSummaryOn} /> Send a daily summary</label>
        <label for="summary-at">at</label>
        <input id="summary-at" type="time" disabled={!isSummaryOn} bind:value={summaryAt} />
        <p class="muted">Default {view.operational.daily_summary.default}.</p>
        {#if view.operational.daily_summary.overridden}
          <button type="button" onclick={() => useDefault('daily_summary')}>
            Use the default for the daily summary
          </button>
        {/if}
      </fieldset>
      <button type="submit">Save settings</button>
    </form>
  {:else if !problem}
    <p>Loading…</p>
  {/if}
</section>

<section aria-labelledby="bootstrap-heading">
  <h2 id="bootstrap-heading">From the file and environment</h2>
  <p class="muted">
    These change only in the settings file or the environment, and apply at the next start.
  </p>
  {#if view}
    <dl>
      {#each Object.entries(view.bootstrap) as [key, value] (key)}
        <dt>{key}</dt>
        <dd>{bootstrapValue(value)}</dd>
      {/each}
    </dl>
  {/if}
</section>

<section aria-labelledby="doctor-heading">
  <h2 id="doctor-heading">Diagnostics</h2>
  <form onsubmit={runDoctor}>
    <label for="doctor-stream">Also resolve and probe</label>
    <select id="doctor-stream" bind:value={doctorStream}>
      <option value="">No stream</option>
      {#each streams as stream (stream.id)}
        <option value={String(stream.id)}>{stream.title}</option>
      {/each}
    </select>
    <button type="submit" disabled={isRunning}>
      {isRunning ? 'Running diagnostics…' : 'Run diagnostics'}
    </button>
  </form>
  {#if doctorProblem}
    <p role="alert">{doctorProblem}</p>
  {/if}
  {#if doctor}
    {@const report = doctor.doctor}
    <h3>Tools</h3>
    <table>
      <thead>
        <tr><th scope="col">Tool</th><th scope="col">Path</th><th scope="col">Version</th></tr>
      </thead>
      <tbody>
        {#each report.tools as tool (tool.name)}
          <tr>
            <th scope="row">{tool.name}</th>
            <td>{tool.path ?? 'not found'}</td>
            <td>{tool.version ?? '-'}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <h3>Network</h3>
    <dl>
      <dt>IPv6 route</dt>
      <dd>{report.has_ipv6_route ? 'yes' : 'no'}</dd>
      <dt>Policy</dt>
      <dd>{report.policy}</dd>
      <dt>Family in use</dt>
      <dd>{report.active_family}</dd>
      <dt>Last switch</dt>
      <dd>
        {#if report.last_switch}
          {inJapan(report.last_switch.at * 1000)}: {report.last_switch.from} to {report.last_switch
            .to}, {report.last_switch.reason}
        {:else}
          none since the service started
        {/if}
      </dd>
      <dt>Request budget</dt>
      <dd>
        {report.limits.concurrency} in flight, {report.limits.requests_per_second} starts per second
      </dd>
    </dl>
    {#if report.resolution.length > 0}
      <h3>URL resolution</h3>
      <ul>
        {#each report.resolution as check (check.family)}
          <li>
            {check.family}: {check.resolved
              ? 'resolved'
              : `failed, ${check.error ?? 'no reason given'}`}
          </li>
        {/each}
      </ul>
    {/if}
    {#if doctor.probe}
      <h3>Probe</h3>
      {#if typeof doctor.probe.error === 'string'}
        <p>{doctor.probe.error}</p>
      {:else}
        <dl>
          <dt>Title</dt>
          <dd>{doctor.probe.title}</dd>
          <dt>Head sq</dt>
          <dd>{doctor.probe.head_sq}</dd>
          <dt>Fragment length</dt>
          <dd>{doctor.probe.fragment_seconds ?? '-'} s</dd>
          <dt>Formats rejected</dt>
          <dd>{Array.isArray(doctor.probe.rejected) ? doctor.probe.rejected.length : 0}</dd>
        </dl>
      {/if}
    {/if}
  {/if}
</section>
