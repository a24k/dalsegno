<script lang="ts">
  import { Feedback } from '../lib/feedback.svelte'
  import FeedbackMessages from '../lib/FeedbackMessages.svelte'
  import { Clock } from '../lib/live-list.svelte'
  import { router } from '../lib/router.svelte'
  import { suggestions, toggle, type BarName, type Selection } from '../lib/selection'
  import { isOpen } from '../lib/types'
  import Candidates from './change/Candidates.svelte'
  import Checks from './change/Checks.svelte'
  import Clips from './change/Clips.svelte'
  import { ChangeDetail } from './change/detail.svelte'
  import Dismiss from './change/Dismiss.svelte'
  import Header from './change/Header.svelte'
  import SelectionForm from './change/Selection.svelte'
  import Strip from './change/Strip.svelte'

  const detail = new ChangeDetail(Number(router.route.params.id))
  const feedback = new Feedback()
  const clock = new Clock()

  let selection = $state<Selection | null>(null)

  $effect(() => detail.start())
  $effect(() => clock.start())

  function choose(bar: BarName, index: number): void {
    selection = toggle(selection, bar, index)
  }

  function made(): void {
    selection = null
    void detail.load()
  }
</script>

{#if detail.isMissing || Number.isNaN(detail.id)}
  <h1>Not found</h1>
  <p>There is no such change. <a href="/">Dashboard</a></p>
{:else if detail.change && detail.comparison}
  {@const change = detail.change}
  {@const comparison = detail.comparison}
  <h1>Change {change.id}</h1>
  <FeedbackMessages {feedback} />
  {#if detail.error}
    <p role="alert">{detail.error}</p>
  {/if}
  <Header {change} streamTitle={detail.streamTitle} now={clock.now} />
  <Strip {comparison} {selection} toggle={choose} />
  <SelectionForm changeId={change.id} {comparison} {selection} {feedback} {made} />
  <Candidates
    changeId={change.id}
    candidates={suggestions(comparison, isOpen(change))}
    {feedback}
    {made}
  />
  <Clips clips={change.clips} />
  <Checks checks={comparison.checks} />
  {#if isOpen(change)}
    <Dismiss changeId={change.id} {feedback} dismissed={(next) => void detail.show(next)} />
  {/if}
{:else if detail.error}
  <p role="alert">{detail.error}</p>
{:else}
  <p>Loading…</p>
{/if}
