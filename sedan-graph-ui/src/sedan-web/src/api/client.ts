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
import { useAuthStore } from '../store/auth'

const BASE_URL = import.meta.env.VITE_API_URL ?? ''

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
  const { token, setRequiresAuth } = useAuthStore.getState()
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(init?.headers as Record<string, string> | undefined),
  }
  if (token) {
    headers['Authorization'] = 'Bearer ' + token
  }

  const url = `${BASE_URL}${path}`
  const response = await fetch(url, {
    ...init,
    headers,
  })

  if (!response.ok) {
    let errorMessage = `HTTP ${response.status}`
    let detail: string | undefined
    try {
      const body = await response.json() as { error?: string; detail?: string }
      errorMessage = body.error ?? errorMessage
      detail = body.detail
    } catch {
      // ignore JSON parse error
    }
    if (response.status === 401) {
      setRequiresAuth(true)
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
      body: JSON.stringify(req),
    })
  },

  mutate(graphName: string, req: MutateRequest): Promise<MutateResponse> {
    return request<MutateResponse>(`/api/graphs/${encodeURIComponent(graphName)}/mutate`, {
      method: 'POST',
      body: JSON.stringify(req),
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
