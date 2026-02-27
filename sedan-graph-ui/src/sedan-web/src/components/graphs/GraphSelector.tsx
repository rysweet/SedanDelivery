import { useState, useRef, useEffect } from 'react'
import { ChevronDown, Plus, Trash2, Database, Check, Loader2 } from 'lucide-react'
import { useGraphStore } from '../../store/graph'
import { useGraphList, useCreateGraph, useDeleteGraph } from '../../hooks/useGraphData'
import { Button } from '../ui/Button'

export function GraphSelector() {
  const [open, setOpen] = useState(false)
  const [creating, setCreating] = useState(false)
  const [newGraphName, setNewGraphName] = useState('')
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  const dropdownRef = useRef<HTMLDivElement>(null)

  const currentGraph = useGraphStore((s) => s.currentGraph)
  const setCurrentGraph = useGraphStore((s) => s.setCurrentGraph)
  const setAvailableGraphs = useGraphStore((s) => s.setAvailableGraphs)

  const { data, isLoading, refetch } = useGraphList()
  const createGraph = useCreateGraph()
  const deleteGraph = useDeleteGraph()

  const graphs = data?.graphs ?? []

  // Sync available graphs to store
  useEffect(() => {
    setAvailableGraphs(graphs)
  }, [graphs, setAvailableGraphs])

  // Close on outside click
  useEffect(() => {
    const handleClick = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setOpen(false)
        setCreating(false)
        setConfirmDelete(null)
      }
    }
    if (open) document.addEventListener('mousedown', handleClick)
    return () => document.removeEventListener('mousedown', handleClick)
  }, [open])

  const handleSelect = (name: string) => {
    setCurrentGraph(name)
    setOpen(false)
    setCreating(false)
  }

  const handleCreate = async () => {
    const name = newGraphName.trim()
    if (!name) return
    await createGraph.mutateAsync({ name })
    setCurrentGraph(name)
    setNewGraphName('')
    setCreating(false)
    setOpen(false)
    refetch()
  }

  const handleDelete = async (name: string) => {
    if (confirmDelete !== name) {
      setConfirmDelete(name)
      return
    }
    await deleteGraph.mutateAsync(name)
    if (currentGraph === name) setCurrentGraph(null)
    setConfirmDelete(null)
    refetch()
  }

  return (
    <div ref={dropdownRef} className="relative">
      <Button
        variant="ghost"
        size="sm"
        onClick={() => setOpen((prev) => !prev)}
        className="flex items-center gap-2 text-gray-300 hover:text-white max-w-48"
      >
        <Database size={14} className="shrink-0 text-accent-400" />
        <span className="truncate text-sm">
          {isLoading ? 'Loading...' : currentGraph ?? 'Select graph'}
        </span>
        <ChevronDown
          size={12}
          className={'shrink-0 transition-transform ' + (open ? 'rotate-180' : '')}
        />
      </Button>

      {open && (
        <div className="absolute right-0 top-full mt-1 z-50 bg-surface-100 border border-surface-300 rounded shadow-2xl min-w-52">
          {/* Graph list */}
          <div className="py-1 max-h-64 overflow-y-auto">
            {isLoading ? (
              <div className="flex items-center gap-2 px-3 py-2 text-xs text-gray-500">
                <Loader2 size={12} className="animate-spin" />
                Loading graphs...
              </div>
            ) : graphs.length === 0 ? (
              <div className="px-3 py-2 text-xs text-gray-600">No graphs available</div>
            ) : (
              graphs.map((name) => (
                <div
                  key={name}
                  className="flex items-center group"
                >
                  <button
                    className={
                      'flex-1 flex items-center gap-2 px-3 py-1.5 text-sm text-left hover:bg-surface-200 transition-colors ' +
                      (name === currentGraph ? 'text-accent-400' : 'text-gray-300')
                    }
                    onClick={() => handleSelect(name)}
                  >
                    {name === currentGraph && <Check size={12} className="shrink-0" />}
                    {name !== currentGraph && <span className="w-3" />}
                    <span className="truncate">{name}</span>
                  </button>
                  <button
                    className={
                      'px-2 py-1.5 opacity-0 group-hover:opacity-100 transition-all ' +
                      (confirmDelete === name
                        ? 'text-red-400'
                        : 'text-gray-600 hover:text-red-400')
                    }
                    onClick={(e) => {
                      e.stopPropagation()
                      handleDelete(name)
                    }}
                    title={confirmDelete === name ? 'Confirm delete?' : 'Delete graph'}
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
              ))
            )}
          </div>

          {/* Divider */}
          <div className="border-t border-surface-300" />

          {/* Create new graph */}
          <div className="p-2">
            {creating ? (
              <div className="flex gap-1">
                <input
                  autoFocus
                  type="text"
                  value={newGraphName}
                  onChange={(e) => setNewGraphName(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') handleCreate()
                    if (e.key === 'Escape') {
                      setCreating(false)
                      setNewGraphName('')
                    }
                  }}
                  placeholder="Graph name..."
                  className="flex-1 bg-surface-DEFAULT border border-surface-300 rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 focus:outline-none focus:ring-1 focus:ring-accent-400"
                />
                <Button
                  variant="primary"
                  size="xs"
                  onClick={handleCreate}
                  loading={createGraph.isPending}
                  disabled={!newGraphName.trim()}
                >
                  Create
                </Button>
              </div>
            ) : (
              <button
                className="w-full flex items-center gap-2 px-2 py-1.5 text-xs text-gray-400 hover:text-gray-200 hover:bg-surface-200 rounded transition-colors"
                onClick={() => setCreating(true)}
              >
                <Plus size={12} />
                Create New Graph
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
