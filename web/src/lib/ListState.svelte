<script lang="ts" generics="T extends { id: number }">
  import type { Snippet } from 'svelte'
  import type { LiveList } from './live-list.svelte'

  let {
    list,
    empty,
    items,
    children,
  }: { list: LiveList<T>; empty: string; items?: T[]; children: Snippet<[T[]]> } = $props()

  let shown = $derived(items ?? list.items)
</script>

{#if list.error}
  <p role="alert">{list.error}</p>
{:else if list.isLoading}
  <p>Loading…</p>
{:else if shown.length === 0}
  <p>{empty}</p>
{:else}
  {@render children(shown)}
{/if}
