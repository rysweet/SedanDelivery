import { useEffect, useCallback, useState } from 'react'
import {
  SigmaContainer,
  useRegisterEvents,
  useLoadGraph,
  useSigma,
} from '@react-sigma/core'
import '@react-sigma/core/lib/react-sigma.min.css'
import Graph from 'graphology'
import { useGraphStore } from '../../store/graph'
import { useSigmaEvents } from '../../hooks/useSigma'
import { useLayout } from '../../hooks/useLayout'
import { applySelectionHighlight } from '../../utils/graphology'
import { Minimap } from './Minimap'
import { ContextMenu } from './ContextMenu'
import type { ContextMenuTarget } from './ContextMenu'
import { HoveredNodeTooltip } from './NodeTooltip'
import type { GraphNode } from '../../types/graph'

interface GraphEventsProps {
  onContextMenu: (target: ContextMenuTarget) => void
  onDoubleClickNode: (nodeId: string) => void
}

function GraphEvents({ onContextMenu, onDoubleClickNode }: GraphEventsProps) {
  const registerEvents = useRegisterEvents()
  const sigma = useSigma()
  const activeMode = useGraphStore((s) => s.activeMode)

  useSigmaEvents()
  useLayout()

  useEffect(() => {
    registerEvents({
      doubleClickNode: (event) => {
        event.event.original.preventDefault()
        onDoubleClickNode(event.node)
      },

      rightClickNode: (event) => {
        event.event.original.preventDefault()
        const me = event.event.original as MouseEvent
        onContextMenu({ kind: 'node', nodeId: event.node, x: me.clientX, y: me.clientY })
      },

      rightClickEdge: (event) => {
        event.event.original.preventDefault()
        const me = event.event.original as MouseEvent
        onContextMenu({ kind: 'edge', edgeId: event.edge, x: me.clientX, y: me.clientY })
      },

      rightClickStage: (event) => {
        event.event.original.preventDefault()
        const me = event.event.original as MouseEvent
        onContextMenu({ kind: 'canvas', x: me.clientX, y: me.clientY })
      },
    })
  }, [registerEvents, onContextMenu, onDoubleClickNode])

  // Update cursor based on mode
  useEffect(() => {
    const container = sigma.getContainer()
    if (!container) return
    switch (activeMode) {
      case 'pan':
        container.style.cursor = 'grab'
        break
      case 'addNode':
      case 'addEdge':
        container.style.cursor = 'crosshair'
        break
      default:
        container.style.cursor = 'default'
    }
  }, [sigma, activeMode])

  return null
}

function GraphLoader({ graph }: { graph: Graph }) {
  const loadGraph = useLoadGraph()
  const sigma = useSigma()
  const selectedNodeId = useGraphStore((s) => s.selectedNodeId)
  const selectedEdgeId = useGraphStore((s) => s.selectedEdgeId)

  useEffect(() => {
    loadGraph(graph)
  }, [graph, loadGraph])

  // Apply selection highlights when selection changes
  useEffect(() => {
    applySelectionHighlight(graph, selectedNodeId, selectedEdgeId)
    sigma.refresh()
  }, [graph, selectedNodeId, selectedEdgeId, sigma])

  return null
}

interface GraphCanvasProps {
  graph: Graph
  onExpandNode: (nodeId: string) => void
  onEditNode: (nodeId: string) => void
  onDeleteNode: (nodeId: string) => void
  onEditEdge: (edgeId: string) => void
  onDeleteEdge: (edgeId: string) => void
  onAddNodeAtPosition: (x: number, y: number) => void
  onFitView: () => void
}

export function GraphCanvas({
  graph,
  onExpandNode,
  onEditNode,
  onDeleteNode,
  onEditEdge,
  onDeleteEdge,
  onAddNodeAtPosition,
  onFitView,
}: GraphCanvasProps) {
  const [contextMenu, setContextMenu] = useState<ContextMenuTarget | null>(null)
  const setSelectedNode = useGraphStore((s) => s.setSelectedNode)
  const setSelectedEdge = useGraphStore((s) => s.setSelectedEdge)

  const handleContextMenu = useCallback((target: ContextMenuTarget) => {
    setContextMenu(target)
  }, [])

  const handleDoubleClickNode = useCallback(
    (nodeId: string) => {
      onExpandNode(nodeId)
    },
    [onExpandNode],
  )

  const getNodeInfo = useCallback(
    (nodeId: string) => {
      if (!graph.hasNode(nodeId)) return null
      const attrs = graph.getNodeAttributes(nodeId)
      const nodeData = attrs.nodeData as GraphNode
      const degree = graph.degree(nodeId)
      const nameProps = ['name', 'title', 'label']
      let nameProperty: string | null = null
      for (const prop of nameProps) {
        const val = nodeData.properties[prop]
        if (val !== null && val !== undefined) {
          nameProperty = String(val)
          break
        }
      }
      return {
        x: 0, // Position shown at mouse, not node position
        y: 0,
        label: attrs.label as string,
        labels: nodeData.labels,
        nameProperty,
        degree,
      }
    },
    [graph],
  )

  return (
    <div className="relative w-full h-full bg-surface-DEFAULT">
      <SigmaContainer
        graph={Graph}
        style={{ width: '100%', height: '100%', background: '#0f1117' }}
        settings={{
          renderEdgeLabels: false,
          defaultEdgeColor: '#2d3142',
          defaultNodeColor: '#4FC3F7',
          labelFont: 'Inter, system-ui, sans-serif',
          labelSize: 11,
          labelColor: { color: '#9ca3af' },
          edgeLabelFont: 'Inter, system-ui, sans-serif',
          edgeLabelSize: 10,
          nodeProgramClasses: {},
          edgeProgramClasses: {},
          allowInvalidContainer: true,
        }}
      >
        <GraphLoader graph={graph} />
        <GraphEvents
          onContextMenu={handleContextMenu}
          onDoubleClickNode={handleDoubleClickNode}
        />
        <Minimap />
        <HoveredNodeTooltip getNodeInfo={getNodeInfo} />
      </SigmaContainer>

      {/* Context menu (rendered outside SigmaContainer to avoid z-index issues) */}
      <ContextMenu
        target={contextMenu}
        onClose={() => setContextMenu(null)}
        onExpandNode={onExpandNode}
        onFocusNode={(nodeId) => setSelectedNode(nodeId)}
        onEditNode={(nodeId) => {
          setSelectedNode(nodeId)
          onEditNode(nodeId)
        }}
        onDeleteNode={onDeleteNode}
        onEditEdge={(edgeId) => {
          setSelectedEdge(edgeId)
          onEditEdge(edgeId)
        }}
        onDeleteEdge={onDeleteEdge}
        onAddNodeAtPosition={onAddNodeAtPosition}
        onFitView={onFitView}
      />
    </div>
  )
}
