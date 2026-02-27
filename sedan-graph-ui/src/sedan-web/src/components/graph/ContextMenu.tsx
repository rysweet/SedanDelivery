import { useEffect, useRef } from 'react'
import { clsx } from 'clsx'
import {
  Expand,
  Focus,
  Route,
  Pencil,
  Trash2,
  Minus,
  PlusCircle,
  Fullscreen,
} from 'lucide-react'

export type ContextMenuTarget =
  | { kind: 'node'; nodeId: string; x: number; y: number }
  | { kind: 'edge'; edgeId: string; x: number; y: number }
  | { kind: 'canvas'; x: number; y: number }

interface MenuItem {
  label: string
  icon: React.ReactNode
  onClick: () => void
  danger?: boolean
  separator?: boolean
  comingSoon?: boolean
}

interface ContextMenuProps {
  target: ContextMenuTarget | null
  onClose: () => void
  onExpandNode: (nodeId: string) => void
  onFocusNode: (nodeId: string) => void
  onEditNode: (nodeId: string) => void
  onDeleteNode: (nodeId: string) => void
  onEditEdge: (edgeId: string) => void
  onDeleteEdge: (edgeId: string) => void
  onAddNodeAtPosition: (x: number, y: number) => void
  onFitView: () => void
}

export function ContextMenu({
  target,
  onClose,
  onExpandNode,
  onFocusNode,
  onEditNode,
  onDeleteNode,
  onEditEdge,
  onDeleteEdge,
  onAddNodeAtPosition,
  onFitView,
}: ContextMenuProps) {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const handleClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        onClose()
      }
    }
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('mousedown', handleClick)
    document.addEventListener('keydown', handleKey)
    return () => {
      document.removeEventListener('mousedown', handleClick)
      document.removeEventListener('keydown', handleKey)
    }
  }, [onClose])

  if (!target) return null

  let items: MenuItem[] = []

  if (target.kind === 'node') {
    items = [
      {
        label: 'Expand Neighborhood',
        icon: <Expand size={14} />,
        onClick: () => { onExpandNode(target.nodeId); onClose() },
      },
      {
        label: 'Focus Node',
        icon: <Focus size={14} />,
        onClick: () => { onFocusNode(target.nodeId); onClose() },
      },
      {
        label: 'Shortest Path From Here',
        icon: <Route size={14} />,
        onClick: () => { onClose() },
        comingSoon: true,
      },
      {
        label: 'Edit Properties',
        icon: <Pencil size={14} />,
        onClick: () => { onEditNode(target.nodeId); onClose() },
        separator: true,
      },
      {
        label: 'Delete Node',
        icon: <Trash2 size={14} />,
        onClick: () => { onDeleteNode(target.nodeId); onClose() },
        danger: true,
      },
    ]
  } else if (target.kind === 'edge') {
    items = [
      {
        label: 'Highlight Path',
        icon: <Minus size={14} />,
        onClick: () => { onClose() },
        comingSoon: true,
      },
      {
        label: 'Edit Properties',
        icon: <Pencil size={14} />,
        onClick: () => { onEditEdge(target.edgeId); onClose() },
        separator: true,
      },
      {
        label: 'Delete Edge',
        icon: <Trash2 size={14} />,
        onClick: () => { onDeleteEdge(target.edgeId); onClose() },
        danger: true,
      },
    ]
  } else {
    items = [
      {
        label: 'Add Node Here',
        icon: <PlusCircle size={14} />,
        onClick: () => { onAddNodeAtPosition(target.x, target.y); onClose() },
      },
      {
        label: 'Fit View',
        icon: <Fullscreen size={14} />,
        onClick: () => { onFitView(); onClose() },
      },
    ]
  }

  // Clamp position to viewport
  const x = Math.min(target.x, window.innerWidth - 200)
  const y = Math.min(target.y, window.innerHeight - items.length * 36 - 16)

  return (
    <div
      ref={ref}
      role="menu"
      aria-label="Context menu"
      className="fixed z-50 bg-surface-100 border border-surface-300 rounded shadow-2xl shadow-black/50 py-1 min-w-48"
      style={{ left: x, top: y }}
    >
      {items.map((item, idx) => (
        <div key={idx}>
          {item.separator && idx > 0 && (
            <div className="my-1 border-t border-surface-300" />
          )}
          <button
            role="menuitem"
            disabled={item.comingSoon}
            className={clsx(
              'w-full flex items-center gap-2 px-3 py-1.5 text-xs text-left',
              item.comingSoon
                ? 'opacity-50 cursor-not-allowed text-gray-500'
                : clsx(
                    'hover:bg-surface-200 transition-colors',
                    item.danger ? 'text-red-400 hover:text-red-300' : 'text-gray-300',
                  ),
            )}
            onClick={item.onClick}
          >
            <span className="shrink-0">{item.icon}</span>
            <span className="flex-1">{item.label}</span>
            {item.comingSoon && (
              <span className="ml-auto text-xs text-gray-600">soon</span>
            )}
          </button>
        </div>
      ))}
    </div>
  )
}
