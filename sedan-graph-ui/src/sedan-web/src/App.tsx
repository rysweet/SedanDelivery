import { Suspense, lazy, useCallback, useMemo, useRef, useState } from 'react'
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
import { GraphErrorBoundary } from './components/graph/GraphErrorBoundary'
import { SearchBar } from './components/search/SearchBar'
import { CreateNodeDialog } from './components/dialogs/CreateNodeDialog'
import { CreateEdgeDialog } from './components/dialogs/CreateEdgeDialog'
import { ExportDialog } from './components/dialogs/ExportDialog'
import { ImportDialog } from './components/dialogs/ImportDialog'
import { Toast } from './components/ui/Toast'
import type { GraphNode, GraphEdge } from './types/graph'
import { mergeGraphData, computeLabelCounts } from './utils/graphology'

const LazyCypherEditor = lazy(async () => {
  const module = await import('./components/editor/CypherEditor')
  return { default: module.CypherEditor }
})

function CypherEditorFallback() {
  return (
    <div className="flex h-full flex-col bg-surface-DEFAULT">
      <div className="border-b border-surface-300 bg-surface-50 px-3 py-1.5">
        <span className="text-xs font-semibold uppercase tracking-wide text-gray-400">
          Cypher
        </span>
      </div>
      <div className="flex flex-1 items-center justify-center px-4 text-sm text-gray-500">
        Loading Cypher editor...
      </div>
    </div>
  )
}

interface DialogState {
  createNode: boolean
  createEdge: boolean
  export: boolean
  import: boolean
}

export default function App() {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const toast = useGraphStore((s) => s.toast)
  const dismissToast = useGraphStore((s) => s.dismissToast)
  const selectedNodeId = useGraphStore((s) => s.selectedNodeId)
  const selectedEdgeId = useGraphStore((s) => s.selectedEdgeId)
  const setSelectedNode = useGraphStore((s) => s.setSelectedNode)
  const setSelectedEdge = useGraphStore((s) => s.setSelectedEdge)
  const clearSelection = useGraphStore((s) => s.clearSelection)
  const toggleLeftPanel = useGraphStore((s) => s.toggleLeftPanel)
  const queryPanelOpen = useGraphStore((s) => s.queryPanelOpen)
  const schemaPanelOpen = useGraphStore((s) => s.schemaPanelOpen)

  const graphRef = useRef<Graph>(new Graph({ multi: false, type: 'directed' }))
  const fitViewRef = useRef<(() => void) | null>(null)
  const [graphVersion, setGraphVersion] = useState(0)

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

  const doDeleteNode = useCallback(
    (id: string) => deleteNode.mutate(id, { onSuccess: clearSelection }),
    [deleteNode, clearSelection],
  )

  const doDeleteEdge = useCallback(
    (id: string) => deleteEdge.mutate(id, { onSuccess: clearSelection }),
    [deleteEdge, clearSelection],
  )

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
    if (selectedNodeId) doDeleteNode(selectedNodeId)
    else if (selectedEdgeId) doDeleteEdge(selectedEdgeId)
  }, [selectedNodeId, selectedEdgeId, doDeleteNode, doDeleteEdge])

  const handleSearchResults = useCallback(
    (nodes: GraphNode[], edges: GraphEdge[]) => {
      mergeGraphData(graphRef.current, nodes, edges)
      handleStatsChange(graphRef.current.order, graphRef.current.size)
      setLabelCounts(computeLabelCounts(graphRef.current))

      // Signal GraphContainer to re-render with merged data
      setGraphVersion((v) => v + 1)
    },
    [handleStatsChange],
  )

  // Get selected node/edge data from graph (memoized on selection + graph stats)
  const selectedNodeData = useMemo((): GraphNode | null => {
    if (!selectedNodeId || !graphRef.current.hasNode(selectedNodeId)) return null
    return graphRef.current.getNodeAttributes(selectedNodeId).nodeData as GraphNode
  }, [selectedNodeId, stats])

  const selectedEdgeData = useMemo((): GraphEdge | null => {
    if (!selectedEdgeId || !graphRef.current.hasEdge(selectedEdgeId)) return null
    return graphRef.current.getEdgeAttributes(selectedEdgeId).edgeData as GraphEdge
  }, [selectedEdgeId, stats])

  const nodeDegree = useMemo(() =>
    selectedNodeId && graphRef.current.hasNode(selectedNodeId)
      ? graphRef.current.degree(selectedNodeId) : 0,
    [selectedNodeId, stats],
  )

  // Export data — computed lazily and memoized on graph stats change
  const exportNodes = useMemo((): GraphNode[] => {
    if (!dialogs.export) return []
    const nodes: GraphNode[] = []
    graphRef.current.forEachNode((_id, attrs) => {
      nodes.push(attrs.nodeData as GraphNode)
    })
    return nodes
  }, [dialogs.export, stats])

  const exportEdges = useMemo((): GraphEdge[] => {
    if (!dialogs.export) return []
    const edges: GraphEdge[] = []
    graphRef.current.forEachEdge((_id, attrs) => {
      edges.push(attrs.edgeData as GraphEdge)
    })
    return edges
  }, [dialogs.export, stats])

  useKeyboardShortcuts({
    onFitView: handleFitView,
    onDeleteSelected: handleDeleteSelected,
    onEscape: clearSelection,
  })

  const bottomSheet =
    queryPanelOpen ? (
      <Suspense fallback={<CypherEditorFallback />}>
        <LazyCypherEditor />
      </Suspense>
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
            graphRef={graphRef}
            graphVersion={graphVersion}
            onEditNode={(nodeId) => setSelectedNode(nodeId)}
            onDeleteNode={doDeleteNode}
            onEditEdge={(edgeId) => setSelectedEdge(edgeId)}
            onDeleteEdge={doDeleteEdge}
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
            onDeleteNode={doDeleteNode}
            onDeleteEdge={doDeleteEdge}
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
        nodes={exportNodes}
        edges={exportEdges}
        graphName={currentGraph ?? 'graph'}
      />

      <ImportDialog
        open={dialogs.import}
        onClose={() => closeDialog('import')}
      />

      {toast && (
        <Toast
          message={toast.message}
          type={toast.type}
          onDismiss={dismissToast}
        />
      )}

    </>
  )
}
