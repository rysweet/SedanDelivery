export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue }

export interface GraphNode {
  id: string
  labels: string[]
  properties: Record<string, JsonValue>
}

export interface GraphEdge {
  id: string
  type: string
  source: string
  target: string
  properties: Record<string, JsonValue>
}

export interface GraphSchema {
  labels: Array<{ name: string; count: number }>
  relationshipTypes: Array<{ name: string; count: number }>
}

export interface QueryResult {
  columns: string[]
  rows: Array<Record<string, JsonValue>>
  elapsedMs: number
}

export interface GraphStats {
  nodeCount: number
  edgeCount: number
  labelCount: number
  relTypeCount: number
}

export interface GraphData {
  nodes: GraphNode[]
  edges: GraphEdge[]
}

export interface NeighborsResult {
  nodes: GraphNode[]
  edges: GraphEdge[]
}

export type LayoutAlgorithm = 'forceAtlas2' | 'circular' | 'random'

export type EditMode = 'select' | 'pan' | 'addNode' | 'addEdge'

export interface Action {
  type: 'createNode' | 'deleteNode' | 'createEdge' | 'deleteEdge' | 'updateNode' | 'updateEdge'
  payload: Record<string, JsonValue>
  inverse: Record<string, JsonValue>
}
