import { RefreshCw } from 'lucide-react'
import { useGraphSchema } from '../../hooks/useGraphData'
import { useGraphStore } from '../../store/graph'
import { getLabelColor } from '../../utils/colors'
import { Button } from '../ui/Button'

export function SchemaPanel() {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const { data, isLoading, refetch } = useGraphSchema(currentGraph)

  if (!currentGraph) {
    return (
      <div className="flex items-center justify-center h-full text-gray-600">
        <p className="text-xs">No graph selected</p>
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-surface-300 bg-surface-DEFAULT">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wide">
          Schema
        </h2>
        <Button
          variant="ghost"
          size="xs"
          onClick={() => refetch()}
          disabled={isLoading}
          className="text-gray-500"
          title="Refresh schema"
        >
          <RefreshCw size={12} className={isLoading ? 'animate-spin' : ''} />
        </Button>
      </div>

      <div className="flex-1 overflow-y-auto p-3 grid grid-cols-2 gap-4">
        {/* Node Labels */}
        <div>
          <p className="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">
            Node Labels
          </p>
          {isLoading ? (
            <div className="space-y-1">
              {[...Array(4)].map((_, i) => (
                <div key={i} className="h-5 bg-surface-200 rounded animate-pulse" />
              ))}
            </div>
          ) : (
            <div className="space-y-1">
              {data?.labels.map((entry) => (
                <div
                  key={entry.name}
                  className="flex items-center justify-between text-xs"
                >
                  <div className="flex items-center gap-1.5">
                    <span
                      className="w-2 h-2 rounded-full shrink-0"
                      style={{ backgroundColor: getLabelColor(entry.name) }}
                    />
                    <span className="text-gray-300 font-mono">{entry.name}</span>
                  </div>
                  <span className="text-gray-600 tabular-nums">{entry.count}</span>
                </div>
              ))}
              {data?.labels.length === 0 && (
                <p className="text-xs text-gray-600">No labels</p>
              )}
            </div>
          )}
        </div>

        {/* Relationship Types */}
        <div>
          <p className="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">
            Relationship Types
          </p>
          {isLoading ? (
            <div className="space-y-1">
              {[...Array(3)].map((_, i) => (
                <div key={i} className="h-5 bg-surface-200 rounded animate-pulse" />
              ))}
            </div>
          ) : (
            <div className="space-y-1">
              {data?.relationshipTypes.map((entry) => (
                <div
                  key={entry.name}
                  className="flex items-center justify-between text-xs"
                >
                  <span className="text-accent-400 font-mono">{entry.name}</span>
                  <span className="text-gray-600 tabular-nums">{entry.count}</span>
                </div>
              ))}
              {data?.relationshipTypes.length === 0 && (
                <p className="text-xs text-gray-600">No relationship types</p>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
