import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError, graphsApi } from '../client'

// ── helpers ───────────────────────────────────────────────────────────────────

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

// ── existing contract tests (must stay green) ─────────────────────────────────

describe('API client error handling', () => {
  const fetchMock = vi.fn()

  beforeEach(() => {
    fetchMock.mockReset()
    vi.stubGlobal('fetch', fetchMock)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('prefers the server message field over the error token', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(413, {
        error: 'graph_too_large_for_delete',
        message:
          "graph 'bigdb' is too large for atomic deletion; use selective deletion instead",
      }),
    )

    await expect(graphsApi.delete('bigdb')).rejects.toEqual(
      expect.objectContaining<ApiError>({
        name: 'ApiError',
        status: 413,
        message: "graph 'bigdb' is too large for atomic deletion; use selective deletion instead",
        detail: "graph 'bigdb' is too large for atomic deletion; use selective deletion instead",
      }),
    )
  })

  it('falls back to the legacy detail field when message is absent', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(413, {
        error: 'graph_too_large_for_delete',
        detail: 'legacy detail',
      }),
    )

    await expect(graphsApi.delete('bigdb')).rejects.toEqual(
      expect.objectContaining<ApiError>({
        name: 'ApiError',
        status: 413,
        message: 'legacy detail',
        detail: 'legacy detail',
      }),
    )
  })
})

// ── R-A: new failing tests — empty-string safety (SEC-3) ─────────────────────
//
// These tests define the contract for the || fallback chain required by SEC-3.
// They FAIL with the current ?? implementation because "" ?? x evaluates to "".
// They will PASS once ?? is replaced with || throughout the fallback chain.

describe('R-A — empty-string safety: || must be used instead of ??', () => {
  const fetchMock = vi.fn()

  beforeEach(() => {
    fetchMock.mockReset()
    vi.stubGlobal('fetch', fetchMock)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  // SEC-3: an empty-string message field must not silently blank the error display
  it('falls back to body.error when body.message is an empty string', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(413, {
        error: 'graph_too_large_for_delete',
        message: '',
      }),
    )

    let thrown: unknown
    try {
      await graphsApi.delete('bigdb')
    } catch (err) {
      thrown = err
    }

    expect(thrown).toBeInstanceOf(ApiError)
    const apiErr = thrown as ApiError
    // Must NOT display blank string to the user
    expect(apiErr.message).not.toBe('')
    // Must fall back to the error token, which is better than nothing
    expect(apiErr.message).toBe('graph_too_large_for_delete')
  })

  // SEC-3: an empty-string detail field must not silently blank the error display
  it('falls back to body.error when both body.message and body.detail are empty strings', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(413, {
        error: 'graph_too_large_for_delete',
        message: '',
        detail: '',
      }),
    )

    let thrown: unknown
    try {
      await graphsApi.delete('bigdb')
    } catch (err) {
      thrown = err
    }

    expect(thrown).toBeInstanceOf(ApiError)
    const apiErr = thrown as ApiError
    expect(apiErr.message).not.toBe('')
    expect(apiErr.message).toBe('graph_too_large_for_delete')
  })

  // SEC-3: empty body.error must fall back to the HTTP status sentinel
  it('falls back to HTTP status sentinel when all string fields are empty strings', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(413, {
        error: '',
        message: '',
        detail: '',
      }),
    )

    let thrown: unknown
    try {
      await graphsApi.delete('bigdb')
    } catch (err) {
      thrown = err
    }

    expect(thrown).toBeInstanceOf(ApiError)
    const apiErr = thrown as ApiError
    // Must surface "HTTP 413" rather than a blank string
    expect(apiErr.message).toBe('HTTP 413')
  })
})

// ── R-A: new failing tests — runtime typeof guards (SEC-2) ───────────────────
//
// These tests define the contract for runtime typeof === 'string' guards (SEC-2).
// They FAIL without those guards because the current code spreads non-string
// values directly into ApiError, causing coerced-to-string outputs like "42".
// They will PASS once individual typeof guards are in place before field use.

describe('R-A — typeof guards: non-string response fields must be ignored', () => {
  const fetchMock = vi.fn()

  beforeEach(() => {
    fetchMock.mockReset()
    vi.stubGlobal('fetch', fetchMock)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  // SEC-2: integer in message field must not be coerced to string and displayed
  it('ignores a non-string message field and falls back to body.error', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(413, {
        error: 'graph_too_large_for_delete',
        message: 42, // number — must not be coerced to "42" and displayed
      }),
    )

    let thrown: unknown
    try {
      await graphsApi.delete('bigdb')
    } catch (err) {
      thrown = err
    }

    expect(thrown).toBeInstanceOf(ApiError)
    const apiErr = thrown as ApiError
    // "42" (coerced integer) is not a valid user-facing error message
    expect(apiErr.message).not.toBe('42')
    expect(apiErr.message).toBe('graph_too_large_for_delete')
  })

  // SEC-2: object in message field must not surface as "[object Object]"
  it('ignores an object-valued message field and falls back to body.error', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(413, {
        error: 'graph_too_large_for_delete',
        message: { nested: 'data' }, // object — must not surface as "[object Object]"
      }),
    )

    let thrown: unknown
    try {
      await graphsApi.delete('bigdb')
    } catch (err) {
      thrown = err
    }

    expect(thrown).toBeInstanceOf(ApiError)
    const apiErr = thrown as ApiError
    expect(apiErr.message).not.toBe('[object Object]')
    expect(apiErr.message).toBe('graph_too_large_for_delete')
  })

  // SEC-2: non-string error field must not be coerced to string
  it('ignores a non-string error field and falls back to HTTP status sentinel', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(500, {
        error: { code: 500 }, // object in error field
      }),
    )

    let thrown: unknown
    try {
      await graphsApi.delete('bigdb')
    } catch (err) {
      thrown = err
    }

    expect(thrown).toBeInstanceOf(ApiError)
    const apiErr = thrown as ApiError
    // Must not display "[object Object]"; must fall back to the HTTP sentinel
    expect(apiErr.message).toBe('HTTP 500')
  })
})

// ── Additional contract tests (should pass with current and fixed code) ───────

describe('API client — success and non-error paths', () => {
  const fetchMock = vi.fn()

  beforeEach(() => {
    fetchMock.mockReset()
    vi.stubGlobal('fetch', fetchMock)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('resolves without a value for a 204 No Content response', async () => {
    fetchMock.mockResolvedValue(
      new Response(null, { status: 204 }),
    )
    await expect(graphsApi.delete('empty-graph')).resolves.toBeUndefined()
  })

  it('uses HTTP status sentinel when JSON body is unparseable', async () => {
    fetchMock.mockResolvedValue(
      new Response('not json', {
        status: 503,
        headers: { 'Content-Type': 'text/plain' },
      }),
    )

    let thrown: unknown
    try {
      await graphsApi.delete('bigdb')
    } catch (err) {
      thrown = err
    }

    expect(thrown).toBeInstanceOf(ApiError)
    expect((thrown as ApiError).status).toBe(503)
    expect((thrown as ApiError).message).toBe('HTTP 503')
  })

  it('throws ApiError with status 401 for unauthorized responses', async () => {
    fetchMock.mockResolvedValue(
      jsonResponse(401, { error: 'unauthorized' }),
    )

    let thrown: unknown
    try {
      await graphsApi.delete('protected-graph')
    } catch (err) {
      thrown = err
    }

    expect(thrown).toBeInstanceOf(ApiError)
    expect((thrown as ApiError).status).toBe(401)
    expect((thrown as ApiError).message).toBe('Authentication required')
  })
})
