import { useGraphStore } from '../../store/graph'

interface StatusBarProps {
  nodeCount: number
  edgeCount: number
}

export function StatusBar({ nodeCount, edgeCount }: StatusBarProps) {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const selectedNodeId = useGraphStore((s) => s.selectedNodeId)
  const selectedEdgeId = useGraphStore((s) => s.selectedEdgeId)
  const zoomLevel = useGraphStore((s) => s.zoomLevel)

  const selectionInfo = selectedNodeId
    ? `Node: ${selectedNodeId}`
    : selectedEdgeId
      ? `Edge: ${selectedEdgeId}`
      : null

  return (
    <div className="flex items-center justify-between px-3 h-8 bg-surface-50 border-t border-surface-300 text-xs text-gray-500 shrink-0 font-mono">
      {/* Left: counts */}
      <div className="flex items-center gap-3">
        <span>
          <span className="text-gray-400">{nodeCount}</span>
          <span className="ml-1">nodes</span>
        </span>
        <span>·</span>
        <span>
          <span className="text-gray-400">{edgeCount}</span>
          <span className="ml-1">edges</span>
        </span>
        {selectionInfo && (
          <>
            <span>·</span>
            <span className="text-accent-400">{selectionInfo}</span>
          </>
        )}
      </div>

      {/* Center: graph name */}
      <div className="absolute left-1/2 -translate-x-1/2">
        {currentGraph ? (
          <span className="text-gray-300 font-medium">{currentGraph}</span>
        ) : (
          <span className="text-gray-600">No graph selected</span>
        )}
      </div>

      {/* Right: zoom */}
      <div className="flex items-center gap-2">
        <span>{Math.round(zoomLevel * 100)}%</span>
      </div>
    </div>
  )
}
