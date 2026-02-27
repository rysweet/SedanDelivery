import { describe, it, expect, beforeEach } from 'vitest'
import { useGraphStore } from '../graph'

// Reset store between tests
beforeEach(() => {
  useGraphStore.setState({
    currentGraph: null,
    availableGraphs: [],
    selectedNodeId: null,
    selectedEdgeId: null,
    hoveredNodeId: null,
    leftPanelOpen: true,
    rightPanelOpen: true,
    activeMode: 'select',
    visibleLabels: new Set(),
    currentLayout: 'forceAtlas2',
    layoutRunning: false,
    queryPanelOpen: false,
    schemaPanelOpen: false,
    lastQueryResult: null,
    history: [],
    historyIndex: -1,
    zoomLevel: 1,
  })
})

describe('setCurrentGraph', () => {
  it('sets the current graph', () => {
    useGraphStore.getState().setCurrentGraph('myGraph')
    expect(useGraphStore.getState().currentGraph).toBe('myGraph')
  })

  it('clears selection when switching graphs', () => {
    useGraphStore.setState({ selectedNodeId: 'n1', selectedEdgeId: 'e1' })
    useGraphStore.getState().setCurrentGraph('anotherGraph')
    expect(useGraphStore.getState().selectedNodeId).toBeNull()
    expect(useGraphStore.getState().selectedEdgeId).toBeNull()
  })

  it('resets history when switching graphs', () => {
    useGraphStore.setState({
      history: [{ type: 'createNode', payload: {}, inverse: {} }],
      historyIndex: 0,
    })
    useGraphStore.getState().setCurrentGraph('newGraph')
    expect(useGraphStore.getState().history).toEqual([])
    expect(useGraphStore.getState().historyIndex).toBe(-1)
  })

  it('can set to null', () => {
    useGraphStore.setState({ currentGraph: 'myGraph' })
    useGraphStore.getState().setCurrentGraph(null)
    expect(useGraphStore.getState().currentGraph).toBeNull()
  })
})

describe('setSelectedNode', () => {
  it('sets the selected node', () => {
    useGraphStore.getState().setSelectedNode('node-1')
    expect(useGraphStore.getState().selectedNodeId).toBe('node-1')
  })

  it('clears selected edge when selecting a node', () => {
    useGraphStore.setState({ selectedEdgeId: 'e1' })
    useGraphStore.getState().setSelectedNode('n1')
    expect(useGraphStore.getState().selectedEdgeId).toBeNull()
  })

  it('can deselect by passing null', () => {
    useGraphStore.setState({ selectedNodeId: 'n1' })
    useGraphStore.getState().setSelectedNode(null)
    expect(useGraphStore.getState().selectedNodeId).toBeNull()
  })
})

describe('setSelectedEdge', () => {
  it('sets the selected edge', () => {
    useGraphStore.getState().setSelectedEdge('edge-1')
    expect(useGraphStore.getState().selectedEdgeId).toBe('edge-1')
  })

  it('clears selected node when selecting an edge', () => {
    useGraphStore.setState({ selectedNodeId: 'n1' })
    useGraphStore.getState().setSelectedEdge('e1')
    expect(useGraphStore.getState().selectedNodeId).toBeNull()
  })
})

describe('clearSelection', () => {
  it('clears both node and edge selection', () => {
    useGraphStore.setState({ selectedNodeId: 'n1', selectedEdgeId: 'e1' })
    useGraphStore.getState().clearSelection()
    expect(useGraphStore.getState().selectedNodeId).toBeNull()
    expect(useGraphStore.getState().selectedEdgeId).toBeNull()
  })
})

describe('setMode', () => {
  it('updates active mode', () => {
    useGraphStore.getState().setMode('pan')
    expect(useGraphStore.getState().activeMode).toBe('pan')
  })

  it('can set all valid modes', () => {
    const modes = ['select', 'pan', 'addNode', 'addEdge'] as const
    for (const mode of modes) {
      useGraphStore.getState().setMode(mode)
      expect(useGraphStore.getState().activeMode).toBe(mode)
    }
  })
})

describe('toggleLabel', () => {
  it('adds a label to visible set', () => {
    useGraphStore.getState().toggleLabel('Person')
    expect(useGraphStore.getState().visibleLabels.has('Person')).toBe(true)
  })

  it('removes a label when toggled again', () => {
    useGraphStore.setState({ visibleLabels: new Set(['Person', 'Company']) })
    useGraphStore.getState().toggleLabel('Person')
    expect(useGraphStore.getState().visibleLabels.has('Person')).toBe(false)
    expect(useGraphStore.getState().visibleLabels.has('Company')).toBe(true)
  })
})

describe('setLayout', () => {
  it('updates current layout', () => {
    useGraphStore.getState().setLayout('circular')
    expect(useGraphStore.getState().currentLayout).toBe('circular')
  })
})

describe('toggleQueryPanel', () => {
  it('opens query panel when closed', () => {
    useGraphStore.setState({ queryPanelOpen: false })
    useGraphStore.getState().toggleQueryPanel()
    expect(useGraphStore.getState().queryPanelOpen).toBe(true)
  })

  it('closes query panel when open', () => {
    useGraphStore.setState({ queryPanelOpen: true })
    useGraphStore.getState().toggleQueryPanel()
    expect(useGraphStore.getState().queryPanelOpen).toBe(false)
  })
})

describe('undo/redo history', () => {
  const sampleAction = {
    type: 'createNode' as const,
    payload: { id: 'n1' },
    inverse: { id: 'n1' },
  }

  it('pushAction adds to history', () => {
    useGraphStore.getState().pushAction(sampleAction)
    expect(useGraphStore.getState().history).toHaveLength(1)
    expect(useGraphStore.getState().historyIndex).toBe(0)
  })

  it('undo returns the last action and decrements index', () => {
    useGraphStore.getState().pushAction(sampleAction)
    const action = useGraphStore.getState().undo()
    expect(action).toEqual(sampleAction)
    expect(useGraphStore.getState().historyIndex).toBe(-1)
  })

  it('redo re-applies the undone action', () => {
    useGraphStore.getState().pushAction(sampleAction)
    useGraphStore.getState().undo()
    const action = useGraphStore.getState().redo()
    expect(action).toEqual(sampleAction)
    expect(useGraphStore.getState().historyIndex).toBe(0)
  })

  it('undo returns null when at beginning of history', () => {
    const action = useGraphStore.getState().undo()
    expect(action).toBeNull()
  })

  it('redo returns null when at end of history', () => {
    useGraphStore.getState().pushAction(sampleAction)
    const action = useGraphStore.getState().redo()
    expect(action).toBeNull()
  })

  it('pushAction truncates forward history when branching', () => {
    useGraphStore.getState().pushAction(sampleAction)
    useGraphStore.getState().pushAction({ ...sampleAction, type: 'deleteNode' })
    useGraphStore.getState().undo()

    // Push a new action - should discard the redo action
    const newAction = { ...sampleAction, type: 'createEdge' as const }
    useGraphStore.getState().pushAction(newAction)

    expect(useGraphStore.getState().history).toHaveLength(2)
    expect(useGraphStore.getState().history[1]).toEqual(newAction)
  })
})

describe('setZoomLevel', () => {
  it('updates zoom level', () => {
    useGraphStore.getState().setZoomLevel(2.5)
    expect(useGraphStore.getState().zoomLevel).toBe(2.5)
  })
})

describe('panel toggles', () => {
  it('toggleLeftPanel toggles the panel', () => {
    expect(useGraphStore.getState().leftPanelOpen).toBe(true)
    useGraphStore.getState().toggleLeftPanel()
    expect(useGraphStore.getState().leftPanelOpen).toBe(false)
    useGraphStore.getState().toggleLeftPanel()
    expect(useGraphStore.getState().leftPanelOpen).toBe(true)
  })

  it('toggleRightPanel toggles the panel', () => {
    expect(useGraphStore.getState().rightPanelOpen).toBe(true)
    useGraphStore.getState().toggleRightPanel()
    expect(useGraphStore.getState().rightPanelOpen).toBe(false)
  })
})
