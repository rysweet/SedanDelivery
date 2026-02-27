import { describe, it, expect, beforeEach } from 'vitest'
import Graph from 'graphology'
import {
  buildGraphFromApiData,
  mergeGraphData,
  applyLabelVisibility,
  extractLabels,
} from '../graphology'
import type { GraphNode, GraphEdge } from '../../types/graph'

const sampleNodes: GraphNode[] = [
  { id: 'n1', labels: ['Person'], properties: { name: 'Alice', age: 30 } },
  { id: 'n2', labels: ['Person'], properties: { name: 'Bob', age: 25 } },
  { id: 'n3', labels: ['Company'], properties: { name: 'Acme Corp' } },
]

const sampleEdges: GraphEdge[] = [
  { id: 'e1', type: 'KNOWS', source: 'n1', target: 'n2', properties: { since: 2020 } },
  { id: 'e2', type: 'WORKS_AT', source: 'n1', target: 'n3', properties: {} },
]

describe('buildGraphFromApiData', () => {
  it('creates a graph with correct node count', () => {
    const graph = buildGraphFromApiData(sampleNodes, sampleEdges)
    expect(graph.order).toBe(3)
  })

  it('creates a graph with correct edge count', () => {
    const graph = buildGraphFromApiData(sampleNodes, sampleEdges)
    expect(graph.size).toBe(2)
  })

  it('nodes have x and y coordinates', () => {
    const graph = buildGraphFromApiData(sampleNodes, sampleEdges)
    graph.forEachNode((_id, attrs) => {
      expect(typeof attrs.x).toBe('number')
      expect(typeof attrs.y).toBe('number')
    })
  })

  it('nodes have correct attributes', () => {
    const graph = buildGraphFromApiData(sampleNodes, sampleEdges)
    const attrs = graph.getNodeAttributes('n1')
    expect(attrs.color).toBeTruthy()
    expect(attrs.label).toBeTruthy()
    expect(attrs.hidden).toBe(false)
    expect(attrs.size).toBeGreaterThan(0)
    expect(attrs.nodeData).toEqual(sampleNodes[0])
  })

  it('edges have correct attributes', () => {
    const graph = buildGraphFromApiData(sampleNodes, sampleEdges)
    const attrs = graph.getEdgeAttributes('e1')
    expect(attrs.color).toBeTruthy()
    expect(attrs.hidden).toBe(false)
    expect(attrs.label).toBe('KNOWS')
    expect(attrs.edgeData).toEqual(sampleEdges[0])
  })

  it('handles empty data', () => {
    const graph = buildGraphFromApiData([], [])
    expect(graph.order).toBe(0)
    expect(graph.size).toBe(0)
  })

  it('skips edges with missing source or target nodes', () => {
    const orphanEdge: GraphEdge = {
      id: 'e_orphan',
      type: 'MISSING',
      source: 'nonexistent',
      target: 'n1',
      properties: {},
    }
    const graph = buildGraphFromApiData(sampleNodes, [...sampleEdges, orphanEdge])
    expect(graph.size).toBe(2) // orphan edge not added
  })

  it('uses name property for label if available', () => {
    const nodeWithName: GraphNode = {
      id: 'n_named',
      labels: ['Person'],
      properties: { name: 'Charlie' },
    }
    const graph = buildGraphFromApiData([nodeWithName], [])
    const attrs = graph.getNodeAttributes('n_named')
    expect(attrs.label).toBe('Charlie')
  })

  it('falls back to id for label if no name property', () => {
    const nodeNoName: GraphNode = {
      id: 'n_noname',
      labels: ['Thing'],
      properties: {},
    }
    const graph = buildGraphFromApiData([nodeNoName], [])
    const attrs = graph.getNodeAttributes('n_noname')
    expect(attrs.label).toBe('n_noname')
  })
})

describe('mergeGraphData', () => {
  let graph: Graph

  beforeEach(() => {
    graph = buildGraphFromApiData(sampleNodes.slice(0, 2), sampleEdges.slice(0, 1))
  })

  it('adds new nodes without removing existing ones', () => {
    mergeGraphData(graph, sampleNodes.slice(2), [])
    expect(graph.order).toBe(3)
  })

  it('does not duplicate existing nodes', () => {
    mergeGraphData(graph, sampleNodes, [])
    expect(graph.order).toBe(3)
  })

  it('adds new edges', () => {
    mergeGraphData(graph, sampleNodes.slice(2), sampleEdges.slice(1))
    expect(graph.size).toBe(2)
  })
})

describe('applyLabelVisibility', () => {
  it('hides nodes of hidden labels', () => {
    const graph = buildGraphFromApiData(sampleNodes, sampleEdges)
    applyLabelVisibility(graph, new Set(['Person']))

    const n1Hidden = graph.getNodeAttribute('n1', 'hidden') as boolean
    const n2Hidden = graph.getNodeAttribute('n2', 'hidden') as boolean
    const n3Hidden = graph.getNodeAttribute('n3', 'hidden') as boolean

    expect(n1Hidden).toBe(true)
    expect(n2Hidden).toBe(true)
    expect(n3Hidden).toBe(false)
  })

  it('hides edges connected to hidden nodes', () => {
    const graph = buildGraphFromApiData(sampleNodes, sampleEdges)
    applyLabelVisibility(graph, new Set(['Company']))

    // e2 connects n1 (Person) to n3 (Company, hidden)
    const e2Hidden = graph.getEdgeAttribute('e2', 'hidden') as boolean
    expect(e2Hidden).toBe(true)

    // e1 connects two Person nodes (both visible)
    const e1Hidden = graph.getEdgeAttribute('e1', 'hidden') as boolean
    expect(e1Hidden).toBe(false)
  })

  it('shows all nodes when hidden labels set is empty', () => {
    const graph = buildGraphFromApiData(sampleNodes, sampleEdges)
    // First hide, then show all
    applyLabelVisibility(graph, new Set(['Person']))
    applyLabelVisibility(graph, new Set())

    graph.forEachNode((_id, attrs) => {
      expect(attrs.hidden).toBe(false)
    })
  })
})

describe('extractLabels', () => {
  it('extracts all unique labels', () => {
    const labels = extractLabels(sampleNodes)
    expect(labels).toContain('Person')
    expect(labels).toContain('Company')
    expect(labels).toHaveLength(2)
  })

  it('returns sorted labels', () => {
    const labels = extractLabels(sampleNodes)
    const sorted = [...labels].sort()
    expect(labels).toEqual(sorted)
  })

  it('handles empty input', () => {
    expect(extractLabels([])).toEqual([])
  })

  it('handles nodes with multiple labels', () => {
    const multiLabelNode: GraphNode = {
      id: 'n_multi',
      labels: ['Person', 'Employee', 'Manager'],
      properties: {},
    }
    const labels = extractLabels([multiLabelNode])
    expect(labels).toHaveLength(3)
    expect(labels).toContain('Person')
    expect(labels).toContain('Employee')
    expect(labels).toContain('Manager')
  })
})
