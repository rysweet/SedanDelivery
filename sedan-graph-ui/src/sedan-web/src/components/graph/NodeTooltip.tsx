import { useGraphStore } from '../../store/graph'
import { Badge } from '../ui/Badge'
import { getLabelColor } from '../../utils/colors'

interface NodeTooltipProps {
  nodeId: string
  x: number
  y: number
  label: string
  labels: string[]
  nameProperty: string | null
  degree: number
}

export function NodeTooltip({
  x,
  y,
  label,
  labels,
  nameProperty,
  degree,
}: NodeTooltipProps) {
  // Clamp to viewport
  const tooltipX = Math.min(x + 16, window.innerWidth - 240)
  const tooltipY = Math.min(y + 16, window.innerHeight - 120)

  return (
    <div
      role="tooltip"
      className="fixed z-40 bg-surface-100 border border-surface-300 rounded shadow-xl px-3 py-2 pointer-events-none min-w-40 max-w-56"
      style={{ left: tooltipX, top: tooltipY }}
    >
      {/* Labels */}
      <div className="flex flex-wrap gap-1 mb-1.5">
        {labels.map((l) => (
          <Badge key={l} variant="custom" color={getLabelColor(l)}>
            {l}
          </Badge>
        ))}
      </div>

      {/* Display name */}
      {nameProperty && (
        <p className="text-sm font-medium text-gray-200 truncate">{nameProperty}</p>
      )}

      {/* Label (fallback) */}
      {!nameProperty && (
        <p className="text-sm font-medium text-gray-200 truncate">{label}</p>
      )}

      {/* Degree */}
      <p className="text-xs text-gray-500 mt-1">
        {degree} connection{degree !== 1 ? 's' : ''}
      </p>
    </div>
  )
}

/**
 * Container that reads hovered node from store and renders tooltip.
 */
interface HoveredNodeTooltipProps {
  getNodeInfo: (nodeId: string) => {
    x: number
    y: number
    label: string
    labels: string[]
    nameProperty: string | null
    degree: number
  } | null
}

export function HoveredNodeTooltip({ getNodeInfo }: HoveredNodeTooltipProps) {
  const hoveredNodeId = useGraphStore((s) => s.hoveredNodeId)

  if (!hoveredNodeId) return null

  const info = getNodeInfo(hoveredNodeId)
  if (!info) return null

  return <NodeTooltip nodeId={hoveredNodeId} {...info} />
}
