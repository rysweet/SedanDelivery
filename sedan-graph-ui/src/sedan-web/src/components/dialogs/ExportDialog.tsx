import { useState } from 'react'
import { Download, FileJson, FileText } from 'lucide-react'
import { Modal } from '../ui/Modal'
import { Button } from '../ui/Button'
import { exportToJson, exportNodesToCSV, exportEdgesToCSV, downloadFile } from '../../utils/export'
import type { GraphNode, GraphEdge } from '../../types/graph'

type ExportFormat = 'json' | 'nodes-csv' | 'edges-csv'

interface ExportDialogProps {
  open: boolean
  onClose: () => void
  nodes: GraphNode[]
  edges: GraphEdge[]
  graphName: string
}

const FORMAT_OPTIONS: { id: ExportFormat; label: string; description: string; icon: React.ReactNode }[] = [
  {
    id: 'json',
    label: 'JSON (full graph)',
    description: 'Exports nodes and edges as a single JSON file',
    icon: <FileJson size={16} />,
  },
  {
    id: 'nodes-csv',
    label: 'CSV (nodes only)',
    description: 'Exports node data as a CSV spreadsheet',
    icon: <FileText size={16} />,
  },
  {
    id: 'edges-csv',
    label: 'CSV (edges only)',
    description: 'Exports edge data as a CSV spreadsheet',
    icon: <FileText size={16} />,
  },
]

export function ExportDialog({ open, onClose, nodes, edges, graphName }: ExportDialogProps) {
  const [selectedFormat, setSelectedFormat] = useState<ExportFormat>('json')

  const handleExport = () => {
    const timestamp = new Date().toISOString().slice(0, 19).replace(/[T:]/g, '-')
    const safeName = graphName.replace(/[^a-zA-Z0-9-_]/g, '_')

    switch (selectedFormat) {
      case 'json': {
        const content = exportToJson(nodes, edges)
        downloadFile(content, `${safeName}-${timestamp}.json`, 'application/json')
        break
      }
      case 'nodes-csv': {
        const content = exportNodesToCSV(nodes)
        downloadFile(content, `${safeName}-nodes-${timestamp}.csv`, 'text/csv')
        break
      }
      case 'edges-csv': {
        const content = exportEdgesToCSV(edges)
        downloadFile(content, `${safeName}-edges-${timestamp}.csv`, 'text/csv')
        break
      }
    }
    onClose()
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Export Graph"
      description={`Exporting "${graphName}" — ${nodes.length} nodes, ${edges.length} edges`}
      size="sm"
      footer={
        <>
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            size="sm"
            onClick={handleExport}
            icon={<Download size={14} />}
          >
            Export
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2">
        {FORMAT_OPTIONS.map((opt) => (
          <button
            key={opt.id}
            className={
              'flex items-start gap-3 px-3 py-2.5 rounded border transition-colors text-left ' +
              (selectedFormat === opt.id
                ? 'border-accent-400 bg-accent-400/10'
                : 'border-surface-300 hover:border-surface-400 hover:bg-surface-100')
            }
            onClick={() => setSelectedFormat(opt.id)}
          >
            <span
              className={
                'shrink-0 mt-0.5 ' +
                (selectedFormat === opt.id ? 'text-accent-400' : 'text-gray-500')
              }
            >
              {opt.icon}
            </span>
            <div>
              <p
                className={
                  'text-sm font-medium ' +
                  (selectedFormat === opt.id ? 'text-accent-400' : 'text-gray-300')
                }
              >
                {opt.label}
              </p>
              <p className="text-xs text-gray-500 mt-0.5">{opt.description}</p>
            </div>
          </button>
        ))}
      </div>
    </Modal>
  )
}
