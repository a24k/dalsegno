<script lang="ts">
  import { holes, period } from '../lib/format'
  import { Feedback } from '../lib/feedback.svelte'
  import FeedbackMessages from '../lib/FeedbackMessages.svelte'
  import ListState from '../lib/ListState.svelte'
  import { LiveList } from '../lib/live-list.svelte'
  import StreamControls from '../lib/StreamControls.svelte'
  import type { Stream } from '../lib/types'
  import AddStream from './stream/AddStream.svelte'

  const LIST_LIMIT = 200

  const streams = new LiveList<Stream>('/streams', 'stream.state', 'oldest-first', LIST_LIMIT)
  const feedback = new Feedback()

  $effect(() => streams.start())
</script>

<h1>Streams</h1>

<FeedbackMessages {feedback} />

<ListState list={streams} empty="No streams yet. Add one below.">
  {#snippet children(items)}
    <table>
      <thead>
        <tr>
          <th scope="col">Stream</th>
          <th scope="col">Video ID</th>
          <th scope="col">State</th>
          <th scope="col">Period</th>
          <th scope="col">Holes</th>
          <th scope="col"><span class="visually-hidden">Actions</span></th>
        </tr>
      </thead>
      <tbody>
        {#each items as stream (stream.id)}
          <tr>
            <th scope="row"><a href="/streams/{stream.id}">{stream.title}</a></th>
            <td>{stream.video_id}</td>
            <td>
              <span class="state" data-state={stream.state}>{stream.state}</span>
              {#if stream.state_reason}({stream.state_reason}){/if}
            </td>
            <td>{period(stream.period)}</td>
            <td>{holes(stream.open_fragments, stream.lost_fragments)}</td>
            <td>
              <StreamControls {stream} {feedback} changed={() => void streams.load()} />
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/snippet}
</ListState>

<AddStream added={() => void streams.load()} />
