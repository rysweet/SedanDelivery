import { useEffect, useRef, useState, useCallback } from 'react'
import Graph from 'graphology'
import { useGraphStore } from '../../store/graph'
import { useGraphData } from '../../hooks/useGraphData'
import { graphDataApi } from '../../api/client'
import {
  buildGraphFromApiData,
  mergeGraphData,
  applyLabelVisibility,
  extractLabels,
} from '../../utils/graphology'
import { GraphCanvas } from './GraphCanvas'
import type { GraphNode } from '../../types/graph'

interface GraphContainerProps {
  onEditNode: (nodeId: string) => void
  onDeleteNode: (nodeId: string) => void
  onEditEdge: (edgeId: string) => void
  onDeleteEdge: (edgeId: string) => void
  onAddNodeAtPosition: (x: number, y: number) => void
  onFitView: () => void
  onGraphReady: (graph: Graph) => void
  onStatsChange: (nodeCount: number, edgeCount: number) => void
}

/**
 * Manages the Graphology Graph instance and keeps it in sync with API data.
 * Passes the graph instance down to GraphCanvas for rendering.
 */
export function GraphContainer({
  onEditNode,
  onDeleteNode,
  onEditEdge,
  onDeleteEdge,
  onAddNodeAtPosition,
  onFitView,
  onGraphReady,
  onStatsChange,
}: GraphContainerProps) {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const visibleLabels = useGraphStore((s) => s.visibleLabels)
  const setVisibleLabels = useGraphStore((s) => s.setVisibleLabels)

  const graphRef = useRef<Graph>(new Graph({ multi: false, type: 'directed' }))
  const [graphInstance, setGraphInstance] = useState<Graph>(graphRef.current)

  const { data, isLoading, error } = useGraphData(currentGraph)

  // Rebuild graph when data changes
  useEffect(() => {
    if (!data) return

    const newGraph = buildGraphFromApiData(data.nodes, data.edges)
    graphRef.current = newGraph
    setGraphInstance(newGraph)
    onGraphReady(newGraph)

    // Initialize all labels as visible
    const labels = extractLabels(data.nodes)
    setVisibleLabels(new Set(labels))

    onStatsChange(newGraph.order, newGraph.size)
  }, [data, onGraphReady, setVisibleLabels, onStatsChange])

  // Apply label visibility filters
  useEffect(() => {
    if (graphRef.current.order === 0) return
    // visibleLabels = labels that ARE shown, empty = all shown
    // We compute hidden labels as those NOT in visibleLabels
    const allLabels = extractLabels(
      graphRef.current
        .nodes()
        .map((id) => graphRef.current.getNodeAttribute(id, 'nodeData') as GraphNode),
    )
    const hiddenLabels = new Set(allLabels.filter((l) => !visibleLabels.has(l)))
    applyLabelVisibility(graphRef.current, hiddenLabels)
    // Force re-render
    setGraphInstance(graphRef.current)
  }, [visibleLabels])

  const handleExpandNode = useCallback(
    async (nodeId: string) => {
      if (!currentGraph) return
      try {
        const result = await graphDataApi.getNeighbors(currentGraph, nodeId)
        mergeGraphData(graphRef.current, result.nodes, result.edges)
        onStatsChange(graphRef.current.order, graphRef.current.size)
        setGraphInstance(graphRef.current)
      } catch {
        // Silently ignore network errors for expansion
      }
    },
    [currentGraph, onStatsChange],
  )

  if (!currentGraph) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500">
        <div className="text-center">
          <div className="text-6xl mb-4 opacity-20">◈</div>
          <p className="text-lg font-medium text-gray-400">No graph selected</p>
          <p className="text-sm mt-1">Select or create a graph to get started</p>
        </div>
      </div>
    )
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500">
        <div className="text-center">
          <div className="animate-spin rounded-full h-12 w-12 border-2 border-surface-300 border-t-accent-400 mx-auto mb-4" />
          <p className="text-sm">Loading graph...</p>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500">
        <div className="text-center">
          <p className="text-base font-medium text-red-400">Failed to load graph</p>
          <p className="text-sm mt-1 text-gray-500">{String(error)}</p>
        </div>
      </div>
    )
  }

  if (graphInstance.order === 0) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500">
        <div className="text-center">
          <div className="text-6xl mb-4 opacity-20">○</div>
          <p className="text-lg font-medium text-gray-400">Graph is empty</p>
          <p className="text-sm mt-1">Add nodes to get started</p>
        </div>
      </div>
    )
  }

  return (
    <GraphCanvas
      graph={graphInstance}
      onExpandNode={handleExpandNode}
      onEditNode={onEditNode}
      onDeleteNode={onDeleteNode}
      onEditEdge={onEditEdge}
      onDeleteEdge={onDeleteEdge}
      onAddNodeAtPosition={onAddNodeAtPosition}
      onFitView={onFitView}
    />
  )
}
