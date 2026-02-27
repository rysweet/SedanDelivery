import type { GraphNode, GraphEdge, JsonValue } from '../types/graph'

interface GraphExportData {
  nodes: GraphNode[]
  edges: GraphEdge[]
  exportedAt: string
  version: string
}

/**
 * Exports graph data as a JSON string.
 */
export function exportToJson(nodes: GraphNode[], edges: GraphEdge[]): string {
  const data: GraphExportData = {
    nodes,
    edges,
    exportedAt: new Date().toISOString(),
    version: '1.0',
  }
  return JSON.stringify(data, null, 2)
}

/**
 * Exports nodes as a CSV string.
 */
export function exportNodesToCSV(nodes: GraphNode[]): string {
  if (nodes.length === 0) return 'id,labels\n'

  // Collect all property keys
  const propKeys = new Set<string>()
  for (const node of nodes) {
    for (const key of Object.keys(node.properties)) {
      propKeys.add(key)
    }
  }

  const headers = ['id', 'labels', ...Array.from(propKeys).sort()]
  const lines = [headers.map(csvEscape).join(',')]

  for (const node of nodes) {
    const row = [
      node.id,
      node.labels.join(';'),
      ...Array.from(propKeys)
        .sort()
        .map((key) => {
          const val = node.properties[key]
          return val === undefined ? '' : formatValue(val)
        }),
    ]
    lines.push(row.map(csvEscape).join(','))
  }

  return lines.join('\n')
}

/**
 * Exports edges as a CSV string.
 */
export function exportEdgesToCSV(edges: GraphEdge[]): string {
  if (edges.length === 0) return 'id,type,source,target\n'

  // Collect all property keys
  const propKeys = new Set<string>()
  for (const edge of edges) {
    for (const key of Object.keys(edge.properties)) {
      propKeys.add(key)
    }
  }

  const headers = ['id', 'type', 'source', 'target', ...Array.from(propKeys).sort()]
  const lines = [headers.map(csvEscape).join(',')]

  for (const edge of edges) {
    const row = [
      edge.id,
      edge.type,
      edge.source,
      edge.target,
      ...Array.from(propKeys)
        .sort()
        .map((key) => {
          const val = edge.properties[key]
          return val === undefined ? '' : formatValue(val)
        }),
    ]
    lines.push(row.map(csvEscape).join(','))
  }

  return lines.join('\n')
}

/**
 * Downloads a string as a file in the browser.
 */
export function downloadFile(content: string, filename: string, mimeType: string): void {
  const blob = new Blob([content], { type: mimeType })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

function formatValue(val: JsonValue): string {
  if (val === null) return ''
  if (typeof val === 'object') return JSON.stringify(val)
  return String(val)
}

function csvEscape(value: string): string {
  if (value.includes(',') || value.includes('"') || value.includes('\n')) {
    return `"${value.replace(/"/g, '""')}"`
  }
  return value
}
