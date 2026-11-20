<script lang="ts">
  import { ago, dvrClock, inJapan } from '../../lib/format'
  import { isOpen, type ChangeDetail } from '../../lib/types'

  let {
    change,
    streamTitle,
    now,
  }: { change: ChangeDetail; streamTitle: string | null; now: number } = $props()
</script>

<p>
  <span class="state" data-state={change.state}>{change.state}</span>
  <a href="/streams/{change.stream_id}">{streamTitle ?? `Stream ${change.stream_id}`}</a>
  <span class="muted">Detected {inJapan(change.detected_at)}, {ago(change.detected_at, now)}</span>
</p>
{#if isOpen(change) && change.dvr_deadline_at !== null}
  <p>{dvrClock(change.dvr_deadline_at, now)}</p>
{/if}
