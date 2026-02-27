import { useState, useCallback, useRef } from 'react'
import Editor, { type OnMount } from '@monaco-editor/react'
import type { editor } from 'monaco-editor'
import { Play, X, AlignLeft, CheckCircle2, AlertCircle } from 'lucide-react'
import { useGraphStore } from '../../store/graph'
import { useQuery_, useMutate } from '../../hooks/useGraphData'
import { Button } from '../ui/Button'
import type { QueryResult } from '../../types/graph'
import type { MutateResponse } from '../../types/api'

const CYPHER_KEYWORDS = [
  'MATCH', 'WHERE', 'RETURN', 'CREATE', 'MERGE', 'DELETE', 'DETACH',
  'SET', 'REMOVE', 'WITH', 'UNWIND', 'UNION', 'ORDER', 'BY', 'LIMIT',
  'SKIP', 'AS', 'OPTIONAL', 'CALL', 'YIELD', 'IN', 'NOT', 'AND', 'OR',
  'XOR', 'IS', 'NULL', 'TRUE', 'FALSE', 'CASE', 'WHEN', 'THEN', 'ELSE',
  'END', 'DISTINCT', 'COUNT', 'SUM', 'AVG', 'MIN', 'MAX', 'COLLECT',
]

const MUTATE_PREFIXES = ['CREATE', 'MERGE', 'DELETE', 'DETACH DELETE', 'SET', 'REMOVE']

function isMutationQuery(query: string): boolean {
  const trimmed = query.trim().toUpperCase()
  return MUTATE_PREFIXES.some((prefix) => trimmed.startsWith(prefix))
}

interface QueryResultTableProps {
  result: QueryResult
}

function QueryResultTable({ result }: QueryResultTableProps) {
  if (result.rows.length === 0) {
    return (
      <div className="flex items-center gap-2 px-3 py-2 text-xs text-gray-500">
        <CheckCircle2 size={12} className="text-green-400" />
        No results returned ({result.elapsedMs}ms)
      </div>
    )
  }

  return (
    <div className="overflow-auto flex-1">
      <table className="w-full text-xs border-collapse">
        <thead>
          <tr className="border-b border-surface-300 bg-surface-50 sticky top-0">
            {result.columns.map((col) => (
              <th
                key={col}
                className="text-left px-3 py-1.5 text-gray-400 font-semibold whitespace-nowrap"
              >
                {col}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {result.rows.map((row, i) => (
            <tr
              key={i}
              className="border-b border-surface-200 hover:bg-surface-100 transition-colors"
            >
              {result.columns.map((col) => (
                <td key={col} className="px-3 py-1 text-gray-300 font-mono whitespace-nowrap max-w-xs truncate">
                  {row[col] === null ? (
                    <span className="text-gray-600 italic">null</span>
                  ) : typeof row[col] === 'object' ? (
                    JSON.stringify(row[col])
                  ) : (
                    String(row[col])
                  )}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      <div className="px-3 py-1 text-xs text-gray-600 border-t border-surface-300">
        {result.rows.length} row{result.rows.length !== 1 ? 's' : ''} · {result.elapsedMs}ms
      </div>
    </div>
  )
}

interface MutateResultBadgeProps {
  result: MutateResponse
}

function MutateResultBadge({ result }: MutateResultBadgeProps) {
  const changes = [
    result.nodesCreated > 0 && `+${result.nodesCreated} nodes`,
    result.nodesDeleted > 0 && `-${result.nodesDeleted} nodes`,
    result.edgesCreated > 0 && `+${result.edgesCreated} edges`,
    result.edgesDeleted > 0 && `-${result.edgesDeleted} edges`,
    result.propertiesSet > 0 && `${result.propertiesSet} props`,
  ].filter(Boolean)

  return (
    <div className="flex items-center gap-2 px-3 py-2 text-xs">
      <CheckCircle2 size={12} className="text-green-400" />
      <span className="text-gray-300">
        {changes.length > 0 ? changes.join(' · ') : 'No changes'}
      </span>
      <span className="text-gray-600">· {result.elapsedMs}ms</span>
    </div>
  )
}

export function CypherEditor() {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const setLastQueryResult = useGraphStore((s) => s.setLastQueryResult)
  const [queryValue, setQueryValue] = useState('MATCH (n) RETURN n LIMIT 25')
  const [queryResult, setQueryResult] = useState<QueryResult | null>(null)
  const [mutateResult, setMutateResult] = useState<MutateResponse | null>(null)
  const [execError, setExecError] = useState<string | null>(null)
  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null)
  const queryMutation = useQuery_(currentGraph)
  const mutateMutation = useMutate(currentGraph)

  const isRunning = queryMutation.isPending || mutateMutation.isPending

  const handleEditorMount: OnMount = (editorInstance, monaco) => {
    editorRef.current = editorInstance

    // Register basic Cypher language support
    monaco.languages.register({ id: 'cypher' })
    monaco.languages.setMonarchTokensProvider('cypher', {
      keywords: CYPHER_KEYWORDS,
      tokenizer: {
        root: [
          [
            /[A-Z_][A-Z0-9_]*/,
            {
              cases: {
                '@keywords': 'keyword',
                '@default': 'identifier',
              },
            },
          ],
          [/[a-z_$][\w$]*/, 'identifier'],
          [/'[^']*'/, 'string'],
          [/"[^"]*"/, 'string'],
          [/`[^`]*`/, 'string'],
          [/\$[a-zA-Z_]\w*/, 'variable'],
          [/\/\/.*$/, 'comment'],
          [/\/\*/, 'comment', '@comment'],
          [/\d+(\.\d+)?/, 'number'],
        ],
        comment: [
          [/\*\//, 'comment', '@pop'],
          [/./, 'comment'],
        ],
      },
    })

    monaco.languages.setLanguageConfiguration('cypher', {
      comments: {
        lineComment: '//',
        blockComment: ['/*', '*/'],
      },
      brackets: [
        ['(', ')'],
        ['[', ']'],
        ['{', '}'],
      ],
      autoClosingPairs: [
        { open: '(', close: ')' },
        { open: '[', close: ']' },
        { open: '{', close: '}' },
        { open: "'", close: "'" },
        { open: '"', close: '"' },
        { open: '`', close: '`' },
      ],
    })

    // Ctrl+Enter to execute
    editorInstance.addCommand(
      monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter,
      handleExecute,
    )
  }

  const handleExecute = useCallback(async () => {
    const query = (editorRef.current?.getValue() ?? queryValue).trim()
    if (!query || !currentGraph) return

    setExecError(null)
    setQueryResult(null)
    setMutateResult(null)

    try {
      if (isMutationQuery(query)) {
        const result = await mutateMutation.mutateAsync({ statement: query })
        setMutateResult(result)
      } else {
        const result = await queryMutation.mutateAsync({ query })
        setQueryResult(result)
        setLastQueryResult(result)
      }
    } catch (err) {
      setExecError(err instanceof Error ? err.message : 'Query failed')
    }
  }, [queryValue, currentGraph, queryMutation, mutateMutation, setLastQueryResult])

  const handleClear = () => {
    editorRef.current?.setValue('')
    setQueryValue('')
    setQueryResult(null)
    setMutateResult(null)
    setExecError(null)
  }

  const handleFormat = () => {
    editorRef.current?.getAction('editor.action.formatDocument')?.run()
  }

  return (
    <div className="flex flex-col h-full bg-surface-DEFAULT">
      {/* Toolbar */}
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-surface-300 bg-surface-50">
        <span className="text-xs font-semibold text-gray-400 uppercase tracking-wide mr-2">
          Cypher
        </span>
        <Button
          variant="primary"
          size="xs"
          onClick={handleExecute}
          loading={isRunning}
          disabled={!currentGraph}
          icon={<Play size={12} />}
        >
          Execute
        </Button>
        <Button variant="ghost" size="xs" onClick={handleClear} className="text-gray-500">
          <X size={12} />
        </Button>
        <Button variant="ghost" size="xs" onClick={handleFormat} className="text-gray-500">
          <AlignLeft size={12} />
        </Button>
        <span className="ml-auto text-xs text-gray-600">Ctrl+Enter to run</span>
      </div>

      {/* Editor */}
      <div className="flex-1 min-h-0">
        <Editor
          height="100%"
          language="cypher"
          value={queryValue}
          onChange={(v) => setQueryValue(v ?? '')}
          onMount={handleEditorMount}
          theme="vs-dark"
          options={{
            fontSize: 13,
            fontFamily: '"Fira Code", "Cascadia Code", Menlo, monospace',
            minimap: { enabled: false },
            lineNumbers: 'on',
            scrollBeyondLastLine: false,
            wordWrap: 'on',
            padding: { top: 8, bottom: 8 },
            suggest: { showKeywords: true },
            quickSuggestions: true,
            tabSize: 2,
          }}
        />
      </div>

      {/* Results area */}
      {(queryResult ?? mutateResult ?? execError) && (
        <div className="shrink-0 h-32 border-t border-surface-300 overflow-hidden flex flex-col">
          {execError && (
            <div className="flex items-start gap-2 px-3 py-2 text-xs text-red-400">
              <AlertCircle size={12} className="shrink-0 mt-0.5" />
              <span>{execError}</span>
            </div>
          )}
          {queryResult && <QueryResultTable result={queryResult} />}
          {mutateResult && <MutateResultBadge result={mutateResult} />}
        </div>
      )}
    </div>
  )
}
