import { useState, useRef, useCallback } from 'react'
import { Search, X, Loader2 } from 'lucide-react'
import { useGraphStore } from '../../store/graph'
import { useQuery_ } from '../../hooks/useGraphData'
import type { GraphNode, GraphEdge, JsonValue } from '../../types/graph'

interface SearchBarProps {
  onResultsFound: (nodes: GraphNode[], edges: GraphEdge[]) => void
}

/**
 * Detects whether the input is a label search (e.g. "Person") vs a property search.
 */
function buildSearchQuery(term: string): string {
  // If it looks like a CamelCase label (no spaces, starts with uppercase)
  const trimmed = term.trim()
  const isLabelSearch = /^[A-Z][A-Za-z0-9]*$/.test(trimmed)

  if (isLabelSearch) {
    // Strip backticks to prevent Cypher label injection
    const safeLabelName = trimmed.replace(/`/g, '')
    return `MATCH (n:\`${safeLabelName}\`) RETURN n LIMIT 50`
  }

  return `MATCH (n) WHERE any(prop in keys(n) WHERE toString(n[prop]) CONTAINS $searchTerm) RETURN n LIMIT 50`
}

export function SearchBar({ onResultsFound }: SearchBarProps) {
  const [value, setValue] = useState('')
  const [resultCount, setResultCount] = useState<number | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const queryMutation = useQuery_(currentGraph)

  const handleSearch = useCallback(async () => {
    const term = value.trim()
    if (!term || !currentGraph) return

    const isLabelSearch = /^[A-Z][A-Za-z0-9]*$/.test(term)
    const query = buildSearchQuery(term)
    const parameters: Record<string, JsonValue> = isLabelSearch ? {} : { searchTerm: term }

    try {
      const result = await queryMutation.mutateAsync({ query, parameters })

      // Extract nodes from result rows
      const nodes: GraphNode[] = []
      for (const row of result.rows) {
        for (const value of Object.values(row)) {
          if (
            value !== null &&
            typeof value === 'object' &&
            !Array.isArray(value) &&
            'id' in value &&
            'labels' in value
          ) {
            nodes.push(value as unknown as GraphNode)
          }
        }
      }

      setResultCount(nodes.length)
      onResultsFound(nodes, [])
    } catch {
      setResultCount(0)
    }
  }, [value, currentGraph, queryMutation, onResultsFound])

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      handleSearch()
    }
  }

  const handleClear = () => {
    setValue('')
    setResultCount(null)
    inputRef.current?.focus()
  }

  const isSearching = queryMutation.isPending

  return (
    <div className="relative flex items-center">
      {/* Search icon */}
      <div className="absolute left-3 text-gray-500 pointer-events-none">
        {isSearching ? (
          <Loader2 size={14} className="animate-spin" />
        ) : (
          <Search size={14} />
        )}
      </div>

      {/* Input */}
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(e) => {
          setValue(e.target.value)
          if (!e.target.value) setResultCount(null)
        }}
        onKeyDown={handleKeyDown}
        placeholder="Search by label or property value..."
        disabled={!currentGraph}
        className="w-full bg-surface-100 border border-surface-300 rounded text-sm text-gray-200 placeholder-gray-600 pl-8 pr-16 py-1.5 focus:outline-none focus:ring-1 focus:ring-accent-400 focus:border-accent-400 transition-colors disabled:opacity-40"
        aria-label="Search graph"
      />

      {/* Result count */}
      {resultCount !== null && (
        <span className="absolute right-9 text-xs text-gray-500 pointer-events-none">
          {resultCount} found
        </span>
      )}

      {/* Clear button */}
      {value && (
        <button
          className="absolute right-3 text-gray-500 hover:text-gray-300 transition-colors"
          onClick={handleClear}
          aria-label="Clear search"
        >
          <X size={14} />
        </button>
      )}
    </div>
  )
}
