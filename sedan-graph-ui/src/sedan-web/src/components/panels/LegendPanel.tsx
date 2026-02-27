import { Eye, EyeOff, ChevronLeft } from 'lucide-react'
import { useGraphStore } from '../../store/graph'
import { getLabelColor } from '../../utils/colors'
import { Button } from '../ui/Button'

interface LabelEntry {
  name: string
  count: number
}

interface LegendPanelProps {
  labels: LabelEntry[]
  onCollapse: () => void
}

export function LegendPanel({ labels, onCollapse }: LegendPanelProps) {
  const visibleLabels = useGraphStore((s) => s.visibleLabels)
  const toggleLabel = useGraphStore((s) => s.toggleLabel)
  const setVisibleLabels = useGraphStore((s) => s.setVisibleLabels)

  const allVisible = labels.every((l) => visibleLabels.has(l.name))

  const handleToggleAll = () => {
    if (allVisible) {
      setVisibleLabels(new Set())
    } else {
      setVisibleLabels(new Set(labels.map((l) => l.name)))
    }
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-surface-300">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wide">
          Node Labels
        </h2>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="xs"
            onClick={handleToggleAll}
            className="text-gray-500 hover:text-gray-300"
            title={allVisible ? 'Hide all' : 'Show all'}
          >
            {allVisible ? <EyeOff size={12} /> : <Eye size={12} />}
          </Button>
          <Button
            variant="ghost"
            size="xs"
            onClick={onCollapse}
            className="text-gray-500 hover:text-gray-300"
            title="Collapse panel"
          >
            <ChevronLeft size={12} />
          </Button>
        </div>
      </div>

      {/* Label list */}
      <div className="flex-1 overflow-y-auto py-1">
        {labels.length === 0 ? (
          <p className="text-xs text-gray-600 text-center py-4">No labels</p>
        ) : (
          labels.map((entry) => {
            const color = getLabelColor(entry.name)
            const isVisible = visibleLabels.has(entry.name)

            return (
              <button
                key={entry.name}
                className={
                  'w-full flex items-center gap-2 px-3 py-1.5 text-xs text-left ' +
                  'hover:bg-surface-100 transition-colors group ' +
                  (isVisible ? '' : 'opacity-40')
                }
                onClick={() => toggleLabel(entry.name)}
                title={isVisible ? `Hide ${entry.name}` : `Show ${entry.name}`}
              >
                {/* Color swatch */}
                <span
                  className="shrink-0 w-3 h-3 rounded-full border border-white/20"
                  style={{ backgroundColor: color }}
                />

                {/* Label name */}
                <span className="flex-1 truncate text-gray-300 group-hover:text-gray-200">
                  {entry.name}
                </span>

                {/* Count */}
                <span className="shrink-0 text-gray-600 tabular-nums">{entry.count}</span>

                {/* Visibility icon (shown on hover) */}
                <span className="shrink-0 opacity-0 group-hover:opacity-100 text-gray-500">
                  {isVisible ? <Eye size={11} /> : <EyeOff size={11} />}
                </span>
              </button>
            )
          })
        )}
      </div>

      {/* Footer */}
      <div className="px-3 py-2 border-t border-surface-300 text-xs text-gray-600">
        {labels.length} label{labels.length !== 1 ? 's' : ''}
      </div>
    </div>
  )
}
