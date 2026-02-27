import { create } from 'zustand'
import { subscribeWithSelector } from 'zustand/middleware'
import type { QueryResult, LayoutAlgorithm, EditMode } from '../types/graph'

interface GraphState {
  // Connection
  currentGraph: string | null
  availableGraphs: string[]

  // Canvas state
  selectedNodeId: string | null
  selectedEdgeId: string | null
  hoveredNodeId: string | null

  // UI state
  leftPanelOpen: boolean
  rightPanelOpen: boolean
  activeMode: EditMode

  // Visible labels (for legend panel)
  visibleLabels: Set<string>

  // Layout
  currentLayout: LayoutAlgorithm
  layoutRunning: boolean

  // Query editor
  queryPanelOpen: boolean
  schemaPanelOpen: boolean
  lastQueryResult: QueryResult | null

  // Zoom
  zoomLevel: number

  // Actions
  setCurrentGraph: (name: string | null) => void
  setAvailableGraphs: (graphs: string[]) => void
  setSelectedNode: (id: string | null) => void
  setSelectedEdge: (id: string | null) => void
  setHoveredNode: (id: string | null) => void
  setMode: (mode: EditMode) => void
  toggleLeftPanel: () => void
  toggleRightPanel: () => void
  toggleLabel: (label: string) => void
  setVisibleLabels: (labels: Set<string>) => void
  setLayout: (layout: LayoutAlgorithm) => void
  setLayoutRunning: (running: boolean) => void
  toggleQueryPanel: () => void
  toggleSchemaPanel: () => void
  setLastQueryResult: (result: QueryResult | null) => void
  setZoomLevel: (level: number) => void
  clearSelection: () => void
}

export const useGraphStore = create<GraphState>()(
  subscribeWithSelector((set, get) => ({
    // Initial state
    currentGraph: null,
    availableGraphs: [],
    selectedNodeId: null,
    selectedEdgeId: null,
    hoveredNodeId: null,
    leftPanelOpen: true,
    rightPanelOpen: true,
    activeMode: 'select',
    visibleLabels: new Set<string>(),
    currentLayout: 'forceAtlas2',
    layoutRunning: false,
    queryPanelOpen: false,
    schemaPanelOpen: false,
    lastQueryResult: null,
    zoomLevel: 1,

    // Actions
    setCurrentGraph: (name) =>
      set({
        currentGraph: name,
        selectedNodeId: null,
        selectedEdgeId: null,
        hoveredNodeId: null,
        visibleLabels: new Set<string>(),
        lastQueryResult: null,
      }),

    setAvailableGraphs: (graphs) => set({ availableGraphs: graphs }),

    setSelectedNode: (id) =>
      set({
        selectedNodeId: id,
        selectedEdgeId: id !== null ? null : get().selectedEdgeId,
      }),

    setSelectedEdge: (id) =>
      set({
        selectedEdgeId: id,
        selectedNodeId: id !== null ? null : get().selectedNodeId,
      }),

    setHoveredNode: (id) => set({ hoveredNodeId: id }),

    setMode: (mode) => set({ activeMode: mode }),

    toggleLeftPanel: () => set((s) => ({ leftPanelOpen: !s.leftPanelOpen })),

    toggleRightPanel: () => set((s) => ({ rightPanelOpen: !s.rightPanelOpen })),

    toggleLabel: (label) =>
      set((s) => {
        const next = new Set(s.visibleLabels)
        if (next.has(label)) {
          next.delete(label)
        } else {
          next.add(label)
        }
        return { visibleLabels: next }
      }),

    setVisibleLabels: (labels) => set({ visibleLabels: labels }),

    setLayout: (layout) => set({ currentLayout: layout }),

    setLayoutRunning: (running) => set({ layoutRunning: running }),

    toggleQueryPanel: () =>
      set((s) => ({
        queryPanelOpen: !s.queryPanelOpen,
        schemaPanelOpen: s.queryPanelOpen ? s.schemaPanelOpen : false,
      })),

    toggleSchemaPanel: () =>
      set((s) => ({
        schemaPanelOpen: !s.schemaPanelOpen,
        queryPanelOpen: s.schemaPanelOpen ? s.queryPanelOpen : false,
      })),

    setLastQueryResult: (result) => set({ lastQueryResult: result }),

    setZoomLevel: (level) => set({ zoomLevel: level }),

    clearSelection: () =>
      set({ selectedNodeId: null, selectedEdgeId: null }),
  })),
)
