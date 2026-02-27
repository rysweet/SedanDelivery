import { useEffect, useCallback } from 'react'
import { useGraphStore } from '../../store/graph'

interface AppLayoutProps {
  toolbar: React.ReactNode
  leftPanel: React.ReactNode
  canvas: React.ReactNode
  rightPanel: React.ReactNode
  statusBar: React.ReactNode
  bottomSheet?: React.ReactNode
}

export function AppLayout({
  toolbar,
  leftPanel,
  canvas,
  rightPanel,
  statusBar,
  bottomSheet,
}: AppLayoutProps) {
  const leftPanelOpen = useGraphStore((s) => s.leftPanelOpen)
  const rightPanelOpen = useGraphStore((s) => s.rightPanelOpen)
  const queryPanelOpen = useGraphStore((s) => s.queryPanelOpen)
  const schemaPanelOpen = useGraphStore((s) => s.schemaPanelOpen)

  const showBottomSheet = queryPanelOpen || schemaPanelOpen

  return (
    <div className="flex flex-col h-screen w-screen bg-surface-DEFAULT overflow-hidden">
      {/* Top toolbar */}
      <div className="shrink-0">{toolbar}</div>

      {/* Main content area */}
      <div className="flex flex-1 min-h-0">
        {/* Left panel */}
        <div
          className="shrink-0 transition-all duration-200 overflow-hidden border-r border-surface-300"
          style={{ width: leftPanelOpen ? 240 : 0 }}
        >
          <div className="w-60 h-full overflow-y-auto">{leftPanel}</div>
        </div>

        {/* Center: canvas + bottom sheet */}
        <div className="flex flex-col flex-1 min-w-0 min-h-0">
          {/* Canvas area */}
          <div className="flex-1 min-h-0 relative">{canvas}</div>

          {/* Bottom sheet (query editor / schema) */}
          {showBottomSheet && bottomSheet && (
            <div className="shrink-0 h-72 border-t border-surface-300 overflow-hidden">
              {bottomSheet}
            </div>
          )}
        </div>

        {/* Right panel */}
        <div
          className="shrink-0 transition-all duration-200 overflow-hidden border-l border-surface-300"
          style={{ width: rightPanelOpen ? 300 : 0 }}
        >
          <div className="w-[300px] h-full overflow-y-auto">{rightPanel}</div>
        </div>
      </div>

      {/* Status bar */}
      <div className="shrink-0">{statusBar}</div>
    </div>
  )
}

/**
 * Hook that handles global keyboard shortcuts for the app.
 */
export function useKeyboardShortcuts(callbacks: {
  onFitView: () => void
  onDeleteSelected: () => void
  onEscape: () => void
}) {
  const { onFitView, onDeleteSelected, onEscape } = callbacks

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      // Skip when focused on input/textarea/select
      const target = e.target as HTMLElement
      if (
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.tagName === 'SELECT' ||
        target.isContentEditable
      ) {
        return
      }

      switch (e.key) {
        case 'f':
        case 'F':
          if (!e.ctrlKey && !e.metaKey) {
            e.preventDefault()
            onFitView()
          }
          break
        case 'Delete':
        case 'Backspace':
          e.preventDefault()
          onDeleteSelected()
          break
        case 'Escape':
          e.preventDefault()
          onEscape()
          break
      }
    },
    [onFitView, onDeleteSelected, onEscape],
  )

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [handleKeyDown])
}
