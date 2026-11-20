// Keeping a listed page current with live updates (D-33).

export interface Identified {
  id: number
}

/** How a list is ordered: the jobs and changes lists put the newest first. */
export type Order = 'newest-first' | 'oldest-first'

/**
 * Puts an updated object in its place, or adds a new one where its order puts it, keeping
 * at most `limit`.
 */
export function upsert<T extends Identified>(
  items: readonly T[],
  item: T,
  order: Order,
  limit: number,
): T[] {
  const index = items.findIndex((known) => known.id === item.id)
  if (index >= 0) {
    return items.map((known, position) => (position === index ? item : known))
  }
  const added = order === 'newest-first' ? [item, ...items] : [...items, item]
  return order === 'newest-first' ? added.slice(0, limit) : added.slice(-limit)
}
