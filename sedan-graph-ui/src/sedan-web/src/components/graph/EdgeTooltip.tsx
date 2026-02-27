interface EdgeTooltipProps {
  edgeId: string
  x: number
  y: number
  type: string
  source: string
  target: string
  propertyCount: number
}

export function EdgeTooltip({ x, y, type, source, target, propertyCount }: EdgeTooltipProps) {
  const tooltipX = Math.min(x + 16, window.innerWidth - 240)
  const tooltipY = Math.min(y + 16, window.innerHeight - 100)

  return (
    <div
      role="tooltip"
      className="fixed z-40 bg-surface-100 border border-surface-300 rounded shadow-xl px-3 py-2 pointer-events-none min-w-40"
      style={{ left: tooltipX, top: tooltipY }}
    >
      <p className="text-sm font-medium text-accent-400">{type}</p>
      <p className="text-xs text-gray-400 mt-1 font-mono">
        {source.slice(0, 8)}... → {target.slice(0, 8)}...
      </p>
      {propertyCount > 0 && (
        <p className="text-xs text-gray-500 mt-1">
          {propertyCount} propert{propertyCount !== 1 ? 'ies' : 'y'}
        </p>
      )}
    </div>
  )
}
