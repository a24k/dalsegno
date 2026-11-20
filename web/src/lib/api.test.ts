import { describe, expect, test, vi } from 'vitest'
import { ApiError, createClient, errorMessage } from './api'

function answer(status: number, body: unknown): typeof fetch {
  return vi.fn(async () => new Response(JSON.stringify(body), { status }))
}

describe('createClient', () => {
  test('returns the data and the page of a list', async () => {
    const meta = { total: 1, page: 1, limit: 50 }
    const fetcher = answer(200, { success: true, data: [{ id: 3 }], error: null, meta })
    const page = await createClient(fetcher).get<{ id: number }[]>('/streams')
    expect(page).toEqual({ items: [{ id: 3 }], meta })
    expect(fetcher).toHaveBeenCalledWith('/api/v1/streams', expect.anything())
  })

  test('sends a body as JSON', async () => {
    const fetcher = answer(202, { success: true, data: { job: 9 }, error: null, meta: null })
    const data = await createClient(fetcher).post<{ job: number }>('/streams/3/start', { a: 1 })
    expect(data).toEqual({ job: 9 })
    const init = vi.mocked(fetcher).mock.calls[0][1]
    expect(init?.method).toBe('POST')
    expect(init?.body).toBe('{"a":1}')
  })

  test('puts a body as JSON', async () => {
    const fetcher = answer(200, { success: true, data: { saved: true }, error: null, meta: null })
    const data = await createClient(fetcher).put<{ saved: boolean }>('/settings', { a: null })
    expect(data).toEqual({ saved: true })
    const init = vi.mocked(fetcher).mock.calls[0][1]
    expect(init?.method).toBe('PUT')
    expect(init?.body).toBe('{"a":null}')
  })

  test('patches a body as JSON', async () => {
    const fetcher = answer(200, { success: true, data: { id: 3 }, error: null, meta: null })
    const data = await createClient(fetcher).patch<{ id: number }>('/streams/3', { title: 'x' })
    expect(data).toEqual({ id: 3 })
    const init = vi.mocked(fetcher).mock.calls[0][1]
    expect(init?.method).toBe('PATCH')
    expect(init?.body).toBe('{"title":"x"}')
  })

  test('deletes without a body', async () => {
    const fetcher = answer(200, { success: true, data: { id: 5 }, error: null, meta: null })
    const data = await createClient(fetcher).delete<{ id: number }>('/programs/1/known-clips/5')
    expect(data).toEqual({ id: 5 })
    const init = vi.mocked(fetcher).mock.calls[0][1]
    expect(init?.method).toBe('DELETE')
    expect(init?.body).toBeUndefined()
  })

  test('raises the code and message the service gave', async () => {
    const error = { code: 'not_found', message: 'no such stream' }
    const fetcher = answer(404, { success: false, data: null, error, meta: null })
    await expect(createClient(fetcher).get('/streams/8')).rejects.toEqual(
      new ApiError(404, 'not_found', 'no such stream'),
    )
  })

  test('raises bad_response for an answer that is not an envelope', async () => {
    const fetcher = vi.fn(async () => new Response('<html>', { status: 502 }))
    await expect(createClient(fetcher).get('/jobs')).rejects.toMatchObject({
      status: 502,
      code: 'bad_response',
    })
  })

  test('raises unreachable when the request never arrives', async () => {
    const fetcher = vi.fn(async () => {
      throw new TypeError('network down')
    })
    await expect(createClient(fetcher).get('/jobs')).rejects.toMatchObject({
      status: 0,
      code: 'unreachable',
    })
  })
})

describe('errorMessage', () => {
  test('gives the message of an API error', () => {
    expect(errorMessage(new ApiError(409, 'invalid_request', 'refused'), 'fallback')).toBe(
      'refused',
    )
  })

  test('gives the fallback for anything else', () => {
    expect(errorMessage(new Error('boom'), 'fallback')).toBe('fallback')
  })
})
