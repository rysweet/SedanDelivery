import { useState } from 'react'
import { Plus, Minus } from 'lucide-react'
import { useGraphStore } from '../../store/graph'
import { useCreateNode } from '../../hooks/useGraphData'
import { Modal } from '../ui/Modal'
import { Button } from '../ui/Button'
import { Input } from '../ui/Input'
import type { JsonValue } from '../../types/graph'

interface PropertyField {
  key: string
  value: string
}

interface CreateNodeDialogProps {
  open: boolean
  onClose: () => void
  onCreated?: () => void
}

export function CreateNodeDialog({ open, onClose, onCreated }: CreateNodeDialogProps) {
  const currentGraph = useGraphStore((s) => s.currentGraph)
  const [labelsInput, setLabelsInput] = useState('')
  const [properties, setProperties] = useState<PropertyField[]>([{ key: '', value: '' }])
  const createNode = useCreateNode(currentGraph)

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
    const labels = labelsInput
      .split(',')
      .map((l) => l.trim())
      .filter(Boolean)

    const propsRecord: Record<string, JsonValue> = {}
    for (const { key, value } of properties) {
      if (key.trim()) {
        // Try to parse as JSON, fallback to string
        try {
          propsRecord[key.trim()] = JSON.parse(value) as JsonValue
        } catch {
          propsRecord[key.trim()] = value
        }
      }
    }

    await createNode.mutateAsync({ labels, properties: propsRecord })
    resetForm()
    onCreated?.()
    onClose()
  }

  const resetForm = () => {
    setLabelsInput('')
    setProperties([{ key: '', value: '' }])
  }

  const handleClose = () => {
    resetForm()
    onClose()
  }

  const isValid = labelsInput.trim().length > 0

  return (
    <Modal
      open={open}
      onClose={handleClose}
      title="Create Node"
      description="Add a new node to the graph"
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
            loading={createNode.isPending}
            disabled={!isValid}
          >
            Create Node
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Input
          label="Labels (comma-separated)"
          placeholder="e.g. Person, Employee"
          value={labelsInput}
          onChange={(e) => setLabelsInput(e.target.value)}
        />

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

        {createNode.error && (
          <p className="text-xs text-red-400">
            {createNode.error instanceof Error ? createNode.error.message : 'Failed to create node'}
          </p>
        )}
      </div>
    </Modal>
  )
}
