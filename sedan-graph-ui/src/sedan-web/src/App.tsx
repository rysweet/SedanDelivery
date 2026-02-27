import { Component, useCallback, useRef, useState } from 'react'
import type { ErrorInfo, ReactNode } from 'react'
import Graph from 'graphology'
import { useGraphStore } from './store/graph'
import { useDeleteNode, useDeleteEdge } from './hooks/useGraphData'
import { AppLayout, useKeyboardShortcuts } from './components/layout/AppLayout'
import { Toolbar } from './components/layout/Toolbar'
import { StatusBar } from './components/layout/StatusBar'
import { LegendPanel } from './components/panels/LegendPanel'
import { InspectorPanel } from './components/panels/InspectorPanel'
import { SchemaPanel } from './components/panels/SchemaPanel'
import { GraphContainer } from './components/graph/GraphContainer'
import { SearchBar } from './components/search/SearchBar'
import { CypherEditor } from './components/editor/CypherEditor'
import { CreateNodeDialog } from './components/dialogs/CreateNodeDialog'
import { CreateEdgeDialog } from './components/dialogs/CreateEdgeDialog'
import { ExportDialog } from './components/dialogs/ExportDialog'
import { ImportDialog } from './components/dialogs/ImportDialog'
import type { GraphNode, GraphEdge } from './types/graph'
import { mergeGraphData, computeLabelCounts } from './utils/graphology'

// Error boundary for graph canvas
interface ErrorBoundaryState {
  hasError: boolean
  error: string | null
}

class GraphErrorBoundary extends Component<
  { children: ReactNode },
  ErrorBoundaryState
> {
  constructor(props: { children: ReactNode }) {
    super(props)
    this.state = { hasError: false, error: null }
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error: error.message }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // In production, this would go to an error tracking service
    if (import.meta.env.DEV) {
      console.error('Graph canvas error:', error, info)
    }
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="flex items-center justify-center h-full text-gray-500">
          <div className="text-center">
            <p className="text-base font-medium text-red-400">Graph rendering error</p>
            <p className="text-sm mt-1">{this.state.error}</p>
            <button
              className="mt-3 text-xs text-accent-400 hover:underline"
              onClick={() => this.setState({ hasError: false, error: null })}
            >
              Retry
            </button>
          </div>
        </div>
      )
    }
    return this.props.children
  }
}

interface DialogState {
  createNode: boolean
  createEdge: boolean
  export: boolean
  import: boolean
}

export default function App() {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const selectedNodeId = useGraphStore((s) => s.selectedNodeId)
  const selectedEdgeId = useGraphStore((s) => s.selectedEdgeId)
  const setSelectedNode = useGraphStore((s) => s.setSelectedNode)
  const setSelectedEdge = useGraphStore((s) => s.setSelectedEdge)
  const clearSelection = useGraphStore((s) => s.clearSelection)
  const toggleLeftPanel = useGraphStore((s) => s.toggleLeftPanel)
  const undo = useGraphStore((s) => s.undo)
  const redo = useGraphStore((s) => s.redo)
  const queryPanelOpen = useGraphStore((s) => s.queryPanelOpen)
  const schemaPanelOpen = useGraphStore((s) => s.schemaPanelOpen)

  const graphRef = useRef<Graph>(new Graph({ multi: false, type: 'directed' }))
  const fitViewRef = useRef<(() => void) | null>(null)

  const [dialogs, setDialogs] = useState<DialogState>({
    createNode: false,
    createEdge: false,
    export: false,
    import: false,
  })

  const [stats, setStats] = useState({ nodeCount: 0, edgeCount: 0 })
  const [labelCounts, setLabelCounts] = useState<Array<{ name: string; count: number }>>([])

  const deleteNode = useDeleteNode(currentGraph)
  const deleteEdge = useDeleteEdge(currentGraph)

  const openDialog = (name: keyof DialogState) =>
    setDialogs((prev) => ({ ...prev, [name]: true }))

  const closeDialog = (name: keyof DialogState) =>
    setDialogs((prev) => ({ ...prev, [name]: false }))

  const handleGraphReady = useCallback(
    (graph: Graph) => {
      graphRef.current = graph
      setLabelCounts(computeLabelCounts(graph))
    },
    [],
  )

  const handleStatsChange = useCallback((nodeCount: number, edgeCount: number) => {
    setStats({ nodeCount, edgeCount })
  }, [])

  const handleFitView = useCallback(() => {
    fitViewRef.current?.()
  }, [])

  const handleDeleteSelected = useCallback(() => {
    if (selectedNodeId) {
      deleteNode.mutate(selectedNodeId, {
        onSuccess: () => clearSelection(),
      })
    } else if (selectedEdgeId) {
      deleteEdge.mutate(selectedEdgeId, {
        onSuccess: () => clearSelection(),
      })
    }
  }, [selectedNodeId, selectedEdgeId, deleteNode, deleteEdge, clearSelection])

  const handleSearchResults = useCallback(
    (nodes: GraphNode[], edges: GraphEdge[]) => {
      mergeGraphData(graphRef.current, nodes, edges)
      handleStatsChange(graphRef.current.order, graphRef.current.size)
      setLabelCounts(computeLabelCounts(graphRef.current))
    },
    [handleStatsChange],
  )

  // Get selected node/edge data from graph
  const selectedNodeData: GraphNode | null = (() => {
    if (!selectedNodeId || !graphRef.current.hasNode(selectedNodeId)) return null
    const attrs = graphRef.current.getNodeAttributes(selectedNodeId)
    return attrs.nodeData as GraphNode
  })()

  const selectedEdgeData: GraphEdge | null = (() => {
    if (!selectedEdgeId || !graphRef.current.hasEdge(selectedEdgeId)) return null
    const attrs = graphRef.current.getEdgeAttributes(selectedEdgeId)
    return attrs.edgeData as GraphEdge
  })()

  const nodeDegree = selectedNodeId
    ? graphRef.current.hasNode(selectedNodeId)
      ? graphRef.current.degree(selectedNodeId)
      : 0
    : 0

  // Get all nodes/edges for export
  const getAllNodes = (): GraphNode[] => {
    const nodes: GraphNode[] = []
    graphRef.current.forEachNode((_id, attrs) => {
      nodes.push(attrs.nodeData as GraphNode)
    })
    return nodes
  }

  const getAllEdges = (): GraphEdge[] => {
    const edges: GraphEdge[] = []
    graphRef.current.forEachEdge((_id, attrs) => {
      edges.push(attrs.edgeData as GraphEdge)
    })
    return edges
  }

  useKeyboardShortcuts({
    onFitView: handleFitView,
    onDeleteSelected: handleDeleteSelected,
    onUndo: () => undo(),
    onRedo: () => redo(),
    onEscape: clearSelection,
  })

  const bottomSheet =
    queryPanelOpen ? (
      <CypherEditor />
    ) : schemaPanelOpen ? (
      <SchemaPanel />
    ) : null

  const canvasArea = (
    <div className="flex flex-col h-full">
      {/* Search bar above canvas */}
      <div className="shrink-0 px-2 py-1.5 border-b border-surface-300 bg-surface-50">
        <SearchBar onResultsFound={handleSearchResults} />
      </div>

      {/* Graph canvas */}
      <div className="flex-1 min-h-0">
        <GraphErrorBoundary>
          <GraphContainer
            onEditNode={(nodeId) => setSelectedNode(nodeId)}
            onDeleteNode={(nodeId) => {
              deleteNode.mutate(nodeId, { onSuccess: () => clearSelection() })
            }}
            onEditEdge={(edgeId) => setSelectedEdge(edgeId)}
            onDeleteEdge={(edgeId) => {
              deleteEdge.mutate(edgeId, { onSuccess: () => clearSelection() })
            }}
            onAddNodeAtPosition={() => openDialog('createNode')}
            onFitView={handleFitView}
            onGraphReady={handleGraphReady}
            onStatsChange={handleStatsChange}
          />
        </GraphErrorBoundary>
      </div>
    </div>
  )

  return (
    <>
      <AppLayout
        toolbar={
          <Toolbar
            onFitView={handleFitView}
            onCreateNode={() => openDialog('createNode')}
            onDeleteSelected={handleDeleteSelected}
            onExport={() => openDialog('export')}
            onImport={() => openDialog('import')}
          />
        }
        leftPanel={
          <LegendPanel
            labels={labelCounts}
            onCollapse={toggleLeftPanel}
          />
        }
        canvas={canvasArea}
        rightPanel={
          <InspectorPanel
            selectedNodeData={selectedNodeData}
            selectedEdgeData={selectedEdgeData}
            nodeDegree={nodeDegree}
            graphName={currentGraph}
            onDeleteNode={(id) => {
              deleteNode.mutate(id, { onSuccess: () => clearSelection() })
            }}
            onDeleteEdge={(id) => {
              deleteEdge.mutate(id, { onSuccess: () => clearSelection() })
            }}
            onExpandNode={(nodeId) => {
              setSelectedNode(nodeId)
            }}
          />
        }
        statusBar={
          <StatusBar
            nodeCount={stats.nodeCount}
            edgeCount={stats.edgeCount}
          />
        }
        bottomSheet={bottomSheet}
      />

      {/* Dialogs */}
      <CreateNodeDialog
        open={dialogs.createNode}
        onClose={() => closeDialog('createNode')}
      />

      <CreateEdgeDialog
        open={dialogs.createEdge}
        onClose={() => closeDialog('createEdge')}
        defaultSourceId={selectedNodeId ?? ''}
      />

      <ExportDialog
        open={dialogs.export}
        onClose={() => closeDialog('export')}
        nodes={getAllNodes()}
        edges={getAllEdges()}
        graphName={currentGraph ?? 'graph'}
      />

      <ImportDialog
        open={dialogs.import}
        onClose={() => closeDialog('import')}
      />
    </>
  )
}
