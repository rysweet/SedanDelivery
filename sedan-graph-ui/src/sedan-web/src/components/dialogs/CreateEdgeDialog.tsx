import { useState } from 'react'
import { Plus, Minus } from 'lucide-react'
import { useGraphStore } from '../../store/graph'
import { useCreateEdge } from '../../hooks/useGraphData'
import { Modal } from '../ui/Modal'
import { Button } from '../ui/Button'
import { Input } from '../ui/Input'
import type { JsonValue } from '../../types/graph'

interface PropertyField {
  key: string
  value: string
}

interface CreateEdgeDialogProps {
  open: boolean
  onClose: () => void
  defaultSourceId?: string
  onCreated?: () => void
}

export function CreateEdgeDialog({
  open,
  onClose,
  defaultSourceId = '',
  onCreated,
}: CreateEdgeDialogProps) {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const [edgeType, setEdgeType] = useState('')
  const [sourceId, setSourceId] = useState(defaultSourceId)
  const [targetId, setTargetId] = useState('')
  const [properties, setProperties] = useState<PropertyField[]>([{ key: '', value: '' }])
  const createEdge = useCreateEdge(currentGraph)

  const handleAddProperty = () => {
    setProperties((prev) => [...prev, { key: '', value: '' }])
  }

  const handleRemoveProperty = (idx: number) => {
    setProperties((prev) => prev.filter((_, i) => i !== idx))
  }

  const handlePropertyChange = (idx: number, field: 'key' | 'value', val: string) => {
    setProperties((prev) =>
      prev.map((p, i) => (i === idx ? { ...p, [field]: val } : p)),
    )
  }

  const handleCreate = async () => {
    const propsRecord: Record<string, JsonValue> = {}
    for (const { key, value } of properties) {
      if (key.trim()) {
        try {
          propsRecord[key.trim()] = JSON.parse(value) as JsonValue
        } catch {
          propsRecord[key.trim()] = value
        }
      }
    }

    await createEdge.mutateAsync({
      type: edgeType.trim(),
      source: sourceId.trim(),
      target: targetId.trim(),
      properties: propsRecord,
    })
    resetForm()
    onCreated?.()
    onClose()
  }

  const resetForm = () => {
    setEdgeType('')
    setSourceId(defaultSourceId)
    setTargetId('')
    setProperties([{ key: '', value: '' }])
  }

  const handleClose = () => {
    resetForm()
    onClose()
  }

  const isValid =
    edgeType.trim().length > 0 && sourceId.trim().length > 0 && targetId.trim().length > 0

  return (
    <Modal
      open={open}
      onClose={handleClose}
      title="Create Edge"
      description="Add a new relationship between nodes"
      size="md"
      footer={
        <>
          <Button variant="ghost" size="sm" onClick={handleClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            size="sm"
            onClick={handleCreate}
            loading={createEdge.isPending}
            disabled={!isValid}
          >
            Create Edge
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Input
          label="Relationship Type"
          placeholder="e.g. KNOWS, WORKS_AT"
          value={edgeType}
          onChange={(e) => setEdgeType(e.target.value)}
        />

        <div className="grid grid-cols-2 gap-3">
          <Input
            label="Source Node ID"
            placeholder="Node ID"
            value={sourceId}
            onChange={(e) => setSourceId(e.target.value)}
          />
          <Input
            label="Target Node ID"
            placeholder="Node ID"
            value={targetId}
            onChange={(e) => setTargetId(e.target.value)}
          />
        </div>

        <div>
          <div className="flex items-center justify-between mb-2">
            <label className="text-xs font-medium text-gray-400 uppercase tracking-wide">
              Properties
            </label>
            <Button
              variant="ghost"
              size="xs"
              onClick={handleAddProperty}
              icon={<Plus size={12} />}
              className="text-gray-500"
            >
              Add
            </Button>
          </div>
          <div className="flex flex-col gap-2">
            {properties.map((prop, idx) => (
              <div key={idx} className="flex items-center gap-2">
                <input
                  className="flex-1 bg-surface-DEFAULT border border-surface-300 rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 focus:outline-none focus:ring-1 focus:ring-accent-400"
                  placeholder="key"
                  value={prop.key}
                  onChange={(e) => handlePropertyChange(idx, 'key', e.target.value)}
                />
                <span className="text-gray-600 text-xs">:</span>
                <input
                  className="flex-1 bg-surface-DEFAULT border border-surface-300 rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 focus:outline-none focus:ring-1 focus:ring-accent-400"
                  placeholder="value"
                  value={prop.value}
                  onChange={(e) => handlePropertyChange(idx, 'value', e.target.value)}
                />
                <Button
                  variant="ghost"
                  size="xs"
                  onClick={() => handleRemoveProperty(idx)}
                  disabled={properties.length === 1}
                  className="text-gray-600 hover:text-red-400"
                >
                  <Minus size={12} />
                </Button>
              </div>
            ))}
          </div>
        </div>

        {createEdge.error && (
          <p className="text-xs text-red-400">
            {createEdge.error instanceof Error ? createEdge.error.message : 'Failed to create edge'}
          </p>
        )}
      </div>
    </Modal>
  )
}
