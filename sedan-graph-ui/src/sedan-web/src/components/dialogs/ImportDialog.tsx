import { useState, useRef } from 'react'
import { Upload, FileJson } from 'lucide-react'
import { useGraphStore } from '../../store/graph'
import { importExportApi } from '../../api/client'
import { Modal } from '../ui/Modal'
import { Button } from '../ui/Button'

interface ImportDialogProps {
  open: boolean
  onClose: () => void
  onImported?: () => void
}

export function ImportDialog({ open, onClose, onImported }: ImportDialogProps) {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const [dragOver, setDragOver] = useState(false)
  const [fileName, setFileName] = useState<string | null>(null)
  const [fileContent, setFileContent] = useState<string | null>(null)
  const [importing, setImporting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)

  const handleFile = (file: File) => {
    setError(null)
    const reader = new FileReader()
    reader.onload = (e) => {
      setFileContent(e.target?.result as string)
      setFileName(file.name)
    }
    reader.onerror = () => setError('Failed to read file')
    reader.readAsText(file)
  }

  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault()
    setDragOver(false)
    const file = e.dataTransfer.files[0]
    if (file) handleFile(file)
  }

  const handleImport = async () => {
    if (!fileContent || !currentGraph) return
    setImporting(true)
    setError(null)
    try {
      await importExportApi.importGraph(currentGraph, {
        format: 'json',
        data: fileContent,
      })
      onImported?.()
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Import failed')
    } finally {
      setImporting(false)
    }
  }

  const handleClose = () => {
    setFileName(null)
    setFileContent(null)
    setError(null)
    onClose()
  }

  return (
    <Modal
      open={open}
      onClose={handleClose}
      title="Import Graph"
      description={currentGraph ? `Import into "${currentGraph}"` : 'Select a graph first'}
      size="sm"
      footer={
        <>
          <Button variant="ghost" size="sm" onClick={handleClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            size="sm"
            onClick={handleImport}
            loading={importing}
            disabled={!fileContent || !currentGraph}
            icon={<Upload size={14} />}
          >
            Import
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        {/* Drop zone */}
        <div
          className={
            'border-2 border-dashed rounded-lg p-6 text-center cursor-pointer transition-colors ' +
            (dragOver
              ? 'border-accent-400 bg-accent-400/10'
              : 'border-surface-300 hover:border-surface-400')
          }
          onDrop={handleDrop}
          onDragOver={(e) => { e.preventDefault(); setDragOver(true) }}
          onDragLeave={() => setDragOver(false)}
          onClick={() => fileInputRef.current?.click()}
          role="button"
          tabIndex={0}
          aria-label="Drop file or click to browse"
        >
          <input
            ref={fileInputRef}
            type="file"
            accept=".json"
            className="hidden"
            onChange={(e) => {
              const file = e.target.files?.[0]
              if (file) handleFile(file)
            }}
          />
          {fileName ? (
            <div className="flex items-center justify-center gap-2 text-sm text-gray-300">
              <FileJson size={16} className="text-accent-400" />
              {fileName}
            </div>
          ) : (
            <div className="text-gray-500">
              <Upload size={24} className="mx-auto mb-2 opacity-50" />
              <p className="text-sm">Drop a JSON file here, or click to browse</p>
              <p className="text-xs mt-1 text-gray-600">Supports graph JSON export format</p>
            </div>
          )}
        </div>

        {error && (
          <p className="text-xs text-red-400">{error}</p>
        )}
      </div>
    </Modal>
  )
}
