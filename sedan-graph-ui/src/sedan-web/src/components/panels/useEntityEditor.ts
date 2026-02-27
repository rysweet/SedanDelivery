import { useState, useEffect, useCallback } from 'react'
import type { JsonValue } from '../../types/graph'

interface UseEntityEditorOptions {
  entityId: string
  properties: Record<string, JsonValue>
  onSave: (props: Record<string, JsonValue>) => void
  onDelete: () => void
}

/**
 * Shared hook for node and edge inspector editing state.
 * Manages local property edits, save, and two-click confirm-delete.
 */
export function useEntityEditor({
  entityId,
  properties,
  onSave,
  onDelete,
}: UseEntityEditorOptions) {
  const [localProps, setLocalProps] = useState<Record<string, JsonValue>>(properties)
  const [confirmDelete, setConfirmDelete] = useState(false)

  useEffect(() => {
    setLocalProps(properties)
    setConfirmDelete(false)
  }, [entityId, properties])

  const handlePropChange = useCallback((key: string, value: string) => {
    setLocalProps((prev) => ({ ...prev, [key]: value }))
  }, [])

  const handleSave = useCallback(() => {
    onSave(localProps)
  }, [localProps, onSave])

  const handleDelete = useCallback(() => {
    if (!confirmDelete) {
      setConfirmDelete(true)
      return
    }
    onDelete()
  }, [confirmDelete, onDelete])

  return {
    localProps,
    confirmDelete,
    handlePropChange,
    handleSave,
    handleDelete,
  }
}
