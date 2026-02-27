import { useEffect, useState, useCallback } from 'react'
import type { MutableRefObject } from 'react'
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
  /** Shared graph ref from App — holds the authoritative Graphology instance */
  graphRef: MutableRefObject<Graph>
  /** Incremented by App when external data (e.g. search results) has been merged */
  graphVersion: number
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
  graphRef,
  graphVersion,
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

  const [graphInstance, setGraphInstance] = useState<Graph>(graphRef.current)

  const { data, isLoading, error } = useGraphData(currentGraph)

  // Rebuild or merge graph when API data changes (Fix #92 — preserve positions)
  useEffect(() => {
    if (!data) return

    if (graphRef.current.order === 0) {
      // First load — build a fresh instance with layout positions assigned by FA2
      const newGraph = buildGraphFromApiData(data.nodes, data.edges)
      graphRef.current = newGraph
      setGraphInstance(newGraph)
      onGraphReady(newGraph)

      // Initialize all labels as visible
      const labels = extractLabels(data.nodes)
      setVisibleLabels(new Set(labels))

      onStatsChange(newGraph.order, newGraph.size)
    } else {
      // Subsequent loads (refetch) — merge to preserve FA2 layout positions
      mergeGraphData(graphRef.current, data.nodes, data.edges)
      onStatsChange(graphRef.current.order, graphRef.current.size)
      // Pass same ref — setGraphInstance triggers re-render with updated data
      setGraphInstance(graphRef.current)
    }
  }, [data, onGraphReady, setVisibleLabels, onStatsChange])

  // Re-render canvas when App signals external graph mutations (Fix #82 — search results)
  useEffect(() => {
    if (graphVersion === 0) return
    setGraphInstance(graphRef.current)
  }, [graphVersion])

  // Apply label visibility filters
  useEffect(() => {
    if (graphRef.current.order === 0) return
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
    [currentGraph, graphRef, onStatsChange],
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
