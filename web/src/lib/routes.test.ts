import { describe, expect, test } from 'vitest'
import { resolve } from './routes'

describe('resolve', () => {
  test('finds the dashboard at the root', () => {
    expect(resolve('/')).toEqual({ screen: 'dashboard', params: {} })
  })

  test('finds every screen with its parameters', () => {
    expect(resolve('/streams')).toEqual({ screen: 'streams', params: {} })
    expect(resolve('/streams/3')).toEqual({ screen: 'stream', params: { id: '3' } })
    expect(resolve('/changes/7')).toEqual({ screen: 'change', params: { id: '7' } })
    expect(resolve('/clips/2/edit')).toEqual({ screen: 'clip-edit', params: { id: '2' } })
    expect(resolve('/clips/2/export')).toEqual({ screen: 'clip-export', params: { id: '2' } })
    expect(resolve('/jobs')).toEqual({ screen: 'jobs', params: {} })
    expect(resolve('/settings')).toEqual({ screen: 'settings', params: {} })
  })

  test('ignores a trailing slash', () => {
    expect(resolve('/streams/')).toEqual({ screen: 'streams', params: {} })
  })

  test('decodes a parameter', () => {
    expect(resolve('/streams/a%20b').params).toEqual({ id: 'a b' })
  })

  test('returns not found for a parameter that does not decode', () => {
    expect(resolve('/streams/%E0').screen).toBe('not-found')
  })

  test('returns not found for an unknown path', () => {
    expect(resolve('/clips/2')).toEqual({ screen: 'not-found', params: {} })
    expect(resolve('/nowhere')).toEqual({ screen: 'not-found', params: {} })
  })
})
