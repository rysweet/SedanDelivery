import Graph from 'graphology'
import { circular } from 'graphology-layout'
import type { GraphNode, GraphEdge } from '../types/graph'
import { getNodeColor } from './colors'

export interface SigmaNodeAttributes {
  x: number
  y: number
  size: number
  color: string
  label: string
  hidden: boolean
  originalColor: string
  nodeData: GraphNode
}

export interface SigmaEdgeAttributes {
  color: string
  size: number
  hidden: boolean
  label: string
  edgeData: GraphEdge
}

const DEFAULT_NODE_SIZE = 10
const DEFAULT_EDGE_SIZE = 2

/**
 * Builds a graphology Graph instance from raw API data.
 * Assigns positions using circular layout as initial state.
 */
export function buildGraphFromApiData(
  nodes: GraphNode[],
  edges: GraphEdge[],
): Graph {
  const graph = new Graph({ multi: false, type: 'directed' })

  for (const node of nodes) {
    if (!graph.hasNode(node.id)) {
      const color = getNodeColor(node.labels)
      const label = getNodeLabel(node)
      graph.addNode(node.id, {
        x: Math.random() * 100,
        y: Math.random() * 100,
        size: DEFAULT_NODE_SIZE,
        color,
        label,
        hidden: false,
        originalColor: color,
        nodeData: node,
      } satisfies Omit<SigmaNodeAttributes, 'x' | 'y'> & { x: number; y: number })
    }
  }

  // Apply circular layout to give initial positions
  if (graph.order > 0) {
    circular.assign(graph)
  }

  for (const edge of edges) {
    if (
      !graph.hasEdge(edge.id) &&
      graph.hasNode(edge.source) &&
      graph.hasNode(edge.target)
    ) {
      try {
        graph.addEdgeWithKey(edge.id, edge.source, edge.target, {
          color: '#4a5568',
          size: DEFAULT_EDGE_SIZE,
          hidden: false,
          label: edge.type,
          edgeData: edge,
        } satisfies SigmaEdgeAttributes)
      } catch {
        // Skip duplicate edges
      }
    }
  }

  return graph
}

/**
 * Merges new nodes/edges into an existing graphology Graph without replacing existing ones.
 */
export function mergeGraphData(
  graph: Graph,
  nodes: GraphNode[],
  edges: GraphEdge[],
): void {
  const newNodes: string[] = []

  for (const node of nodes) {
    if (!graph.hasNode(node.id)) {
      const color = getNodeColor(node.labels)
      const label = getNodeLabel(node)
      graph.addNode(node.id, {
        x: Math.random() * 100 - 50,
        y: Math.random() * 100 - 50,
        size: DEFAULT_NODE_SIZE,
        color,
        label,
        hidden: false,
        originalColor: color,
        nodeData: node,
      })
      newNodes.push(node.id)
    }
  }

  for (const edge of edges) {
    if (
      !graph.hasEdge(edge.id) &&
      graph.hasNode(edge.source) &&
      graph.hasNode(edge.target)
    ) {
      try {
        graph.addEdgeWithKey(edge.id, edge.source, edge.target, {
          color: '#4a5568',
          size: DEFAULT_EDGE_SIZE,
          hidden: false,
          label: edge.type,
          edgeData: edge,
        })
      } catch {
        // Skip duplicate edges
      }
    }
  }
}

/**
 * Updates node visibility in the graph based on visible label set.
 * Empty set means all labels are visible.
 */
export function applyLabelVisibility(
  graph: Graph,
  hiddenLabels: Set<string>,
): void {
  graph.forEachNode((nodeId, rawAttrs) => {
    const attrs = rawAttrs as unknown as SigmaNodeAttributes
    const nodeData = attrs.nodeData as GraphNode
    const shouldHide = nodeData.labels.some((l) => hiddenLabels.has(l))
    graph.setNodeAttribute(nodeId, 'hidden', shouldHide)
  })

  graph.forEachEdge((edgeId, _attrs, source, target) => {
    const sourceHidden = graph.getNodeAttribute(source, 'hidden') as boolean
    const targetHidden = graph.getNodeAttribute(target, 'hidden') as boolean
    graph.setEdgeAttribute(edgeId, 'hidden', sourceHidden || targetHidden)
  })
}

/**
 * Highlights the selected node and dims others.
 */
export function applySelectionHighlight(
  graph: Graph,
  selectedNodeId: string | null,
  selectedEdgeId: string | null,
): void {
  if (selectedNodeId === null && selectedEdgeId === null) {
    // Reset all highlights
    graph.forEachNode((nodeId, rawAttrs) => {
      const attrs = rawAttrs as unknown as SigmaNodeAttributes
      graph.setNodeAttribute(nodeId, 'color', attrs.originalColor)
      graph.setNodeAttribute(nodeId, 'size', DEFAULT_NODE_SIZE)
    })
    graph.forEachEdge((edgeId) => {
      graph.setEdgeAttribute(edgeId, 'color', '#4a5568')
      graph.setEdgeAttribute(edgeId, 'size', DEFAULT_EDGE_SIZE)
    })
    return
  }

  if (selectedNodeId !== null) {
    const connectedNodes = new Set<string>([selectedNodeId])
    const connectedEdges = new Set<string>()

    graph.forEachEdge((edgeId, _attrs, source, target) => {
      if (source === selectedNodeId || target === selectedNodeId) {
        connectedNodes.add(source)
        connectedNodes.add(target)
        connectedEdges.add(edgeId)
      }
    })

    graph.forEachNode((nodeId, rawAttrs) => {
      const attrs = rawAttrs as unknown as SigmaNodeAttributes
      if (nodeId === selectedNodeId) {
        graph.setNodeAttribute(nodeId, 'color', attrs.originalColor)
        graph.setNodeAttribute(nodeId, 'size', DEFAULT_NODE_SIZE * 1.5)
      } else if (connectedNodes.has(nodeId)) {
        graph.setNodeAttribute(nodeId, 'color', attrs.originalColor)
        graph.setNodeAttribute(nodeId, 'size', DEFAULT_NODE_SIZE)
      } else {
        graph.setNodeAttribute(nodeId, 'color', '#2d3142')
        graph.setNodeAttribute(nodeId, 'size', DEFAULT_NODE_SIZE * 0.8)
      }
    })

    graph.forEachEdge((edgeId) => {
      if (connectedEdges.has(edgeId)) {
        graph.setEdgeAttribute(edgeId, 'color', '#4FC3F7')
        graph.setEdgeAttribute(edgeId, 'size', DEFAULT_EDGE_SIZE * 1.5)
      } else {
        graph.setEdgeAttribute(edgeId, 'color', '#1e2130')
        graph.setEdgeAttribute(edgeId, 'size', DEFAULT_EDGE_SIZE)
      }
    })
  }
}

/**
 * Computes label counts from all nodes in a graphology Graph instance.
 * Returns a sorted array of { name, count } entries.
 */
export function computeLabelCounts(graph: Graph): Array<{ name: string; count: number }> {
  const labelMap = new Map<string, number>()
  graph.forEachNode((_id, attrs) => {
    const nodeData = attrs.nodeData as GraphNode
    for (const label of nodeData.labels) {
      labelMap.set(label, (labelMap.get(label) ?? 0) + 1)
    }
  })
  return Array.from(labelMap.entries())
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => a.name.localeCompare(b.name))
}

/**
 * Extracts all unique labels from graph nodes.
 */
export function extractLabels(nodes: GraphNode[]): string[] {
  const labels = new Set<string>()
  for (const node of nodes) {
    for (const label of node.labels) {
      labels.add(label)
    }
  }
  return Array.from(labels).sort()
}

/**
 * Returns the display label for a node (using name/title property or ID).
 */
function getNodeLabel(node: GraphNode): string {
  const nameProps = ['name', 'title', 'label', 'id', 'key']
  for (const prop of nameProps) {
    const val = node.properties[prop]
    if (val !== null && val !== undefined) {
      return String(val)
    }
  }
  return node.id
}
