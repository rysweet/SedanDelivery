import type { GraphNode, GraphEdge, GraphSchema, QueryResult, JsonValue } from './graph'

// Request types
export interface CreateNodeRequest {
  labels: string[]
  properties: Record<string, JsonValue>
}

export interface UpdateNodeRequest {
  properties: Record<string, JsonValue>
}

export interface CreateEdgeRequest {
  type: string
  source: string
  target: string
  properties: Record<string, JsonValue>
}

export interface UpdateEdgeRequest {
  properties: Record<string, JsonValue>
}

export interface QueryRequest {
  query: string
  parameters?: Record<string, JsonValue>
}

export interface MutateRequest {
  statement: string
  parameters?: Record<string, JsonValue>
}

export interface CreateGraphRequest {
  name: string
}

export interface ImportGraphRequest {
  format: 'json' | 'csv'
  data: string
}

// Response types
export interface GraphListResponse {
  graphs: string[]
}

export interface GraphDataResponse {
  nodes: GraphNode[]
  edges: GraphEdge[]
}

export interface NeighborsResponse {
  nodes: GraphNode[]
  edges: GraphEdge[]
}

export type SchemaResponse = GraphSchema

export type QueryResponse = QueryResult

export interface MutateResponse {
  nodesCreated: number
  nodesDeleted: number
  edgesCreated: number
  edgesDeleted: number
  propertiesSet: number
  elapsedMs: number
}

export interface ErrorResponse {
  error: string
  detail?: string
  message?: string
}
