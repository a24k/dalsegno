// A typed client for /api/v1. Every response comes in the envelope of docs/design/api.md.

export const API_BASE = '/api/v1'

export interface Meta {
  total: number
  page: number
  limit: number
}

export interface Envelope<T> {
  success: boolean
  data: T
  error: { code: string; message: string } | null
  meta: Meta | null
}

export interface Page<T> {
  items: T
  meta: Meta | null
}

/** A request that failed, with the stable code the API gave or one of its own. */
export class ApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code: string, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

type Fetch = typeof fetch

function isEnvelope(value: unknown): value is Envelope<unknown> {
  return typeof value === 'object' && value !== null && 'success' in value && 'data' in value
}

async function readEnvelope(response: Response): Promise<Envelope<unknown>> {
  const body: unknown = await response.json().catch(() => null)
  if (!isEnvelope(body)) {
    throw new ApiError(response.status, 'bad_response', 'The service sent an unreadable answer.')
  }
  return body
}

export interface Client {
  get<T>(path: string): Promise<Page<T>>
  post<T>(path: string, body?: unknown): Promise<T>
  put<T>(path: string, body: unknown): Promise<T>
}

/** Builds a client. Tests hand it their own fetch. */
export function createClient(fetcher: Fetch = (...args) => fetch(...args)): Client {
  async function request(path: string, init: RequestInit): Promise<Envelope<unknown>> {
    let response: Response
    try {
      response = await fetcher(`${API_BASE}${path}`, init)
    } catch {
      throw new ApiError(0, 'unreachable', 'The service could not be reached.')
    }
    const envelope = await readEnvelope(response)
    if (!envelope.success || !response.ok) {
      const error = envelope.error ?? { code: 'failed', message: 'The request failed.' }
      throw new ApiError(response.status, error.code, error.message)
    }
    return envelope
  }

  function send(path: string, method: string, body: unknown): Promise<Envelope<unknown>> {
    return request(path, {
      method,
      headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
  }

  return {
    async get<T>(path: string): Promise<Page<T>> {
      const envelope = await request(path, { headers: { Accept: 'application/json' } })
      return { items: envelope.data as T, meta: envelope.meta }
    },
    async post<T>(path: string, body?: unknown): Promise<T> {
      return (await send(path, 'POST', body)).data as T
    },
    async put<T>(path: string, body: unknown): Promise<T> {
      return (await send(path, 'PUT', body)).data as T
    },
  }
}

export const api = createClient()
