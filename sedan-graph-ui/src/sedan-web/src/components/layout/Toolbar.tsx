import { useCallback, useState } from 'react'
import {
  MousePointer2,
  Hand,
  Plus,
  Trash2,
  Layout,
  Maximize2,
  Terminal,
  Table,
  Download,
  Upload,
  CircleStop,
  Circle,
  Activity,
  Lock,
} from 'lucide-react'
import { useGraphStore } from '../../store/graph'
import { useAuthStore } from '../../store/auth'
import { Button } from '../ui/Button'
import { Tooltip } from '../ui/Tooltip'
import { GraphSelector } from '../graphs/GraphSelector'
import type { LayoutAlgorithm, EditMode } from '../../types/graph'

interface ToolbarProps {
  onFitView: () => void
  onCreateNode: () => void
  onDeleteSelected: () => void
  onExport: () => void
  onImport: () => void
}

interface ToolbarButtonProps {
  tooltip: string
  onClick: () => void
  active?: boolean
  disabled?: boolean
  children: React.ReactNode
}

function ToolbarButton({ tooltip, onClick, active, disabled, children }: ToolbarButtonProps) {
  return (
    <Tooltip content={tooltip} placement="bottom" delay={300}>
      <Button
        variant="ghost"
        size="xs"
        onClick={onClick}
        disabled={disabled}
        aria-label={tooltip}
        className={
          active
            ? 'bg-accent-400/20 text-accent-400 hover:bg-accent-400/30'
            : 'text-gray-400 hover:text-gray-200'
        }
      >
        {children}
      </Button>
    </Tooltip>
  )
}

function Separator() {
  return <div className="w-px h-5 bg-surface-300 mx-1" aria-hidden="true" />
}

const LAYOUT_LABELS: Record<LayoutAlgorithm, string> = {
  forceAtlas2: 'Force Atlas 2',
  circular: 'Circular',
  random: 'Random',
}

export function Toolbar({
  onFitView,
  onCreateNode,
  onDeleteSelected,
  onExport,
  onImport,
}: ToolbarProps) {
  const activeMode = useGraphStore((s) => s.activeMode)
  const setMode = useGraphStore((s) => s.setMode)
  const currentLayout = useGraphStore((s) => s.currentLayout)
  const setLayout = useGraphStore((s) => s.setLayout)
  const layoutRunning = useGraphStore((s) => s.layoutRunning)
  const setLayoutRunning = useGraphStore((s) => s.setLayoutRunning)
  const toggleQueryPanel = useGraphStore((s) => s.toggleQueryPanel)
  const toggleSchemaPanel = useGraphStore((s) => s.toggleSchemaPanel)
  const queryPanelOpen = useGraphStore((s) => s.queryPanelOpen)
  const schemaPanelOpen = useGraphStore((s) => s.schemaPanelOpen)
  const selectedNodeId = useGraphStore((s) => s.selectedNodeId)
  const selectedEdgeId = useGraphStore((s) => s.selectedEdgeId)

  const token = useAuthStore((s) => s.token)
  const clearToken = useAuthStore((s) => s.clearToken)

  const hasSelection = selectedNodeId !== null || selectedEdgeId !== null

  const handleModeChange = useCallback(
    (mode: EditMode) => {
      setMode(mode)
    },
    [setMode],
  )

  const handleLayoutChange = useCallback(
    (algorithm: LayoutAlgorithm) => {
      setLayout(algorithm)
    },
    [setLayout],
  )

  // Fix #94 — setting layoutRunning=false is observed by useLayout which stops the FA2 worker
  const handleStopLayout = useCallback(() => {
    setLayoutRunning(false)
  }, [setLayoutRunning])

  const [layoutDropdownOpen, setLayoutDropdownOpen] = useState(false)

  return (
    <div className="flex items-center gap-0.5 px-2 h-12 bg-surface-50 border-b border-surface-300 shrink-0">
      {/* Mode tools */}
      <ToolbarButton
        tooltip="Select (S)"
        onClick={() => handleModeChange('select')}
        active={activeMode === 'select'}
      >
        <MousePointer2 size={16} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Pan (P)"
        onClick={() => handleModeChange('pan')}
        active={activeMode === 'pan'}
      >
        <Hand size={16} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Add Node (N)"
        onClick={() => {
          handleModeChange('addNode')
          onCreateNode()
        }}
        active={activeMode === 'addNode'}
      >
        <Plus size={16} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Delete Selected (Del)"
        onClick={onDeleteSelected}
        disabled={!hasSelection}
      >
        <Trash2 size={16} />
      </ToolbarButton>

      <Separator />

      {/* Layout */}
      <div className="flex items-center gap-0.5">
        <div
          className="relative"
          onKeyDown={(e) => { if (e.key === 'Escape') setLayoutDropdownOpen(false) }}
        >
          <ToolbarButton
            tooltip="Layout algorithm"
            onClick={() => setLayoutDropdownOpen(!layoutDropdownOpen)}
          >
            <Layout size={16} />
            <span className="text-xs hidden sm:inline">
              {LAYOUT_LABELS[currentLayout]}
            </span>
          </ToolbarButton>
          {layoutDropdownOpen && (
            <div
              role="menu"
              aria-label="Layout algorithms"
              className="absolute top-full left-0 mt-1 z-50 bg-surface-100 border border-surface-300 rounded shadow-lg py-1 min-w-36"
              onBlur={(e) => {
                if (!e.currentTarget.contains(e.relatedTarget as Node)) setLayoutDropdownOpen(false)
              }}
            >
              {(Object.entries(LAYOUT_LABELS) as [LayoutAlgorithm, string][]).map(
                ([algo, label]) => (
                  <button
                    key={algo}
                    role="menuitemradio"
                    aria-checked={currentLayout === algo}
                    className={
                      'w-full text-left px-3 py-1.5 text-xs hover:bg-surface-200 transition-colors ' +
                      (currentLayout === algo ? 'text-accent-400' : 'text-gray-300')
                    }
                    onClick={() => { handleLayoutChange(algo); setLayoutDropdownOpen(false) }}
                  >
                    {label}
                  </button>
                ),
              )}
            </div>
          )}
        </div>
        {layoutRunning && (
          <ToolbarButton tooltip="Stop layout" onClick={handleStopLayout}>
            <CircleStop size={16} className="text-accent-400 animate-pulse" />
          </ToolbarButton>
        )}
        {!layoutRunning && (
          <ToolbarButton tooltip="Run layout" onClick={() => setLayout(currentLayout)}>
            <Circle size={16} />
          </ToolbarButton>
        )}
      </div>

      <ToolbarButton tooltip="Fit View (F)" onClick={onFitView}>
        <Maximize2 size={16} />
      </ToolbarButton>

      <Separator />

      {/* Panels */}
      <ToolbarButton
        tooltip="Cypher Query Editor"
        onClick={toggleQueryPanel}
        active={queryPanelOpen}
      >
        <Terminal size={16} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Schema View"
        onClick={toggleSchemaPanel}
        active={schemaPanelOpen}
      >
        <Table size={16} />
      </ToolbarButton>

      <Separator />

      {/* Import/Export */}
      <ToolbarButton tooltip="Export Graph" onClick={onExport}>
        <Download size={16} />
      </ToolbarButton>
      <ToolbarButton tooltip="Import Graph" onClick={onImport}>
        <Upload size={16} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Service Status"
        onClick={() => { window.location.hash = '#/status' }}
      >
        <Activity size={16} />
      </ToolbarButton>

      {token && (
        <>
          <Separator />
          <ToolbarButton tooltip="Authenticated — click to disconnect" onClick={clearToken}>
            <Lock size={16} />
          </ToolbarButton>
        </>
      )}

      {/* Graph selector pushed to right */}
      <div className="ml-auto">
        <GraphSelector />
      </div>
    </div>
  )
}
