import type { PublicClientApplication } from '@azure/msal-browser'
import { apiTokenRequest } from '../auth/msalConfig'
import type {
  GraphListResponse,
  GraphDataResponse,
  NeighborsResponse,
  SchemaResponse,
  QueryResponse,
  MutateResponse,
  CreateNodeRequest,
  UpdateNodeRequest,
  CreateEdgeRequest,
  UpdateEdgeRequest,
  QueryRequest,
  MutateRequest,
  CreateGraphRequest,
  ImportGraphRequest,
} from '../types/api'
import type { GraphNode, GraphEdge } from '../types/graph'

const BASE_URL = import.meta.env.VITE_API_URL ?? ''
const isTestAuth = import.meta.env.VITE_E2E_TEST_AUTH === 'true'

// ── MSAL instance management ──────────────────────────────────────────────
let msalInstance: PublicClientApplication | null = null

export function setMsalInstance(instance: PublicClientApplication) {
  msalInstance = instance
}

async function getAccessToken(): Promise<string | null> {
  // In E2E test mode, no MSAL token — use X-Test-Auth header instead
  if (isTestAuth) return null

  if (!msalInstance) return null
  const accounts = msalInstance.getAllAccounts()
  if (accounts.length === 0) return null
  try {
    const result = await msalInstance.acquireTokenSilent({
      ...apiTokenRequest,
      account: accounts[0],
    })
    return result.accessToken
  } catch {
    msalInstance.loginRedirect(apiTokenRequest).catch(() => {})
    return null
  }
}

// ── HTTP client ───────────────────────────────────────────────────────────
class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    public detail?: string,
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const url = `${BASE_URL}${path}`

  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(init?.headers as Record<string, string>),
  }

  if (isTestAuth) {
    // E2E test mode: send X-Test-Auth header instead of JWT
    headers['X-Test-Auth'] = 'e2e-playwright'
  } else {
    // Production: acquire MSAL JWT token
    const token = await getAccessToken()
    if (token) {
      headers['Authorization'] = `Bearer ${token}`
    }
  }

  const response = await fetch(url, {
    ...init,
    headers,
  })

  if (!response.ok) {
    let errorMessage = `HTTP ${response.status}`
    let detail: string | undefined
    try {
      // SEC-2: Use individual runtime typeof guards rather than a compile-time
      // type cast so non-string values (numbers, objects, null) from untrusted
      // JSON are never coerced to strings like "42" or "[object Object]".
      const rawBody = await response.json() as Record<string, unknown>
      const bodyMessage = typeof rawBody.message === 'string' ? rawBody.message : ''
      const bodyError = typeof rawBody.error === 'string' ? rawBody.error : ''
      const bodyDetail = typeof rawBody.detail === 'string' ? rawBody.detail : ''
      // detail is sourced exclusively from the body's `detail` field so it
      // remains supplementary text distinct from the primary message.
      detail = bodyDetail || undefined
      // SEC-3: Use || (logical OR) rather than ?? (nullish coalescing) so that
      // empty strings are treated as absent, preventing a blank error display.
      // Priority: explicit message > machine-readable error token > supplementary detail > HTTP sentinel
      errorMessage = bodyMessage || bodyError || bodyDetail || errorMessage
    } catch {
      // ignore JSON parse error — errorMessage retains the "HTTP N" sentinel
    }
    if (response.status === 401) {
      // Token expired or invalid — MSAL will handle re-auth on next acquireTokenSilent
      throw new ApiError(401, 'Authentication required', detail)
    }
    throw new ApiError(response.status, errorMessage, detail)
  }

  if (response.status === 204) {
    return undefined as T
  }

  return response.json() as Promise<T>
}

// Graph management
export const graphsApi = {
  list(): Promise<GraphListResponse> {
    return request<GraphListResponse>('/api/graphs')
  },

  create(req: CreateGraphRequest): Promise<void> {
    return request<void>('/api/graphs', {
      method: 'POST',
      body: JSON.stringify(req),
    })
  },

  delete(name: string): Promise<void> {
    return request<void>(`/api/graphs/${encodeURIComponent(name)}`, {
      method: 'DELETE',
    })
  },
}

// Graph data
export const graphDataApi = {
  getAll(graphName: string): Promise<GraphDataResponse> {
    return request<GraphDataResponse>(`/api/graphs/${encodeURIComponent(graphName)}/data`)
  },

  getNeighbors(graphName: string, nodeId: string): Promise<NeighborsResponse> {
    return request<NeighborsResponse>(
      `/api/graphs/${encodeURIComponent(graphName)}/neighbors/${encodeURIComponent(nodeId)}`,
    )
  },

  getSchema(graphName: string): Promise<SchemaResponse> {
    return request<SchemaResponse>(`/api/graphs/${encodeURIComponent(graphName)}/schema`)
  },
}

// Node operations
export const nodesApi = {
  create(graphName: string, req: CreateNodeRequest): Promise<GraphNode> {
    return request<GraphNode>(`/api/graphs/${encodeURIComponent(graphName)}/nodes`, {
      method: 'POST',
      body: JSON.stringify(req),
    })
  },

  update(graphName: string, nodeId: string, req: UpdateNodeRequest): Promise<GraphNode> {
    return request<GraphNode>(
      `/api/graphs/${encodeURIComponent(graphName)}/nodes/${encodeURIComponent(nodeId)}`,
      {
        method: 'PUT',
        body: JSON.stringify(req),
      },
    )
  },

  delete(graphName: string, nodeId: string): Promise<void> {
    return request<void>(
      `/api/graphs/${encodeURIComponent(graphName)}/nodes/${encodeURIComponent(nodeId)}`,
      { method: 'DELETE' },
    )
  },
}

// Edge operations
export const edgesApi = {
  create(graphName: string, req: CreateEdgeRequest): Promise<GraphEdge> {
    return request<GraphEdge>(`/api/graphs/${encodeURIComponent(graphName)}/edges`, {
      method: 'POST',
      body: JSON.stringify(req),
    })
  },

  update(graphName: string, edgeId: string, req: UpdateEdgeRequest): Promise<GraphEdge> {
    return request<GraphEdge>(
      `/api/graphs/${encodeURIComponent(graphName)}/edges/${encodeURIComponent(edgeId)}`,
      {
        method: 'PUT',
        body: JSON.stringify(req),
      },
    )
  },

  delete(graphName: string, edgeId: string): Promise<void> {
    return request<void>(
      `/api/graphs/${encodeURIComponent(graphName)}/edges/${encodeURIComponent(edgeId)}`,
      { method: 'DELETE' },
    )
  },
}

// Query operations
export const queryApi = {
  query(graphName: string, req: QueryRequest): Promise<QueryResponse> {
    return request<QueryResponse>(`/api/graphs/${encodeURIComponent(graphName)}/query`, {
      method: 'POST',
      body: JSON.stringify({
        cypher: req.query,
        params: req.parameters,
      }),
    })
  },

  mutate(graphName: string, req: MutateRequest): Promise<MutateResponse> {
    return request<MutateResponse>(`/api/graphs/${encodeURIComponent(graphName)}/mutate`, {
      method: 'POST',
      body: JSON.stringify({
        cypher: req.statement,
        params: req.parameters,
      }),
    })
  },
}

// Import/Export
export const importExportApi = {
  importGraph(graphName: string, req: ImportGraphRequest): Promise<void> {
    return request<void>(`/api/graphs/${encodeURIComponent(graphName)}/import`, {
      method: 'POST',
      body: JSON.stringify(req),
    })
  },
}

export { ApiError }
