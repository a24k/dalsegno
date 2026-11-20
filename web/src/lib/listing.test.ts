import { describe, expect, test } from 'vitest'
import { upsert } from './listing'

const items = [
  { id: 3, state: 'running' },
  { id: 2, state: 'queued' },
]

describe('upsert', () => {
  test('replaces an object in its place', () => {
    const next = upsert(items, { id: 2, state: 'running' }, 'newest-first', 10)

    expect(next).toEqual([
      { id: 3, state: 'running' },
      { id: 2, state: 'running' },
    ])
    expect(items[1].state).toBe('queued')
  })

  test('puts a new object first when the newest come first, within the limit', () => {
    expect(upsert(items, { id: 4, state: 'queued' }, 'newest-first', 2)).toEqual([
      { id: 4, state: 'queued' },
      { id: 3, state: 'running' },
    ])
  })

  test('puts a new object last when the oldest come first', () => {
    const streams = [{ id: 1 }, { id: 2 }]

    expect(upsert(streams, { id: 3 }, 'oldest-first', 10)).toEqual([
      { id: 1 },
      { id: 2 },
      { id: 3 },
    ])
  })
})
