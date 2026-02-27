import { useState, useEffect } from 'react'
import type { JsonValue } from '../../types/graph'

export interface PropertyEditorProps {
  propKey: string
  value: JsonValue
  onChange: (key: string, value: string) => void
  nodeId?: string
}

export function PropertyEditor({ propKey, value, onChange }: PropertyEditorProps) {
  const [localValue, setLocalValue] = useState(String(value ?? ''))

  useEffect(() => {
    setLocalValue(String(value ?? ''))
  }, [value])

  return (
    <div className="flex flex-col gap-1">
      <label className="text-xs text-gray-500 font-mono">{propKey}</label>
      <input
        className="w-full bg-surface-DEFAULT border border-surface-300 rounded px-2 py-1 text-xs text-gray-200 font-mono focus:outline-none focus:ring-1 focus:ring-accent-400 focus:border-accent-400"
        type="text"
        value={localValue}
        onChange={(e) => setLocalValue(e.target.value)}
        onBlur={() => onChange(propKey, localValue)}
      />
    </div>
  )
}

export interface PropertyListProps {
  properties: Record<string, JsonValue>
  onChange: (key: string, value: string) => void
  nodeId: string
}

export function PropertyList({ nodeId, properties, onChange }: PropertyListProps) {
  const entries = Object.entries(properties)

  if (entries.length === 0) {
    return <p className="text-xs text-gray-600 py-2">No properties</p>
  }

  return (
    <div className="flex flex-col gap-2">
      {entries.map(([key, value]) => (
        <PropertyEditor
          key={`${nodeId}-${key}`}
          propKey={key}
          value={value}
          onChange={onChange}
        />
      ))}
    </div>
  )
}
