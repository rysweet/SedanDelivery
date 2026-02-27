import { useState, useEffect } from 'react'
import { Trash2, Save, Network, ChevronRight } from 'lucide-react'
import { useUpdateNode, useDeleteNode, useUpdateEdge, useDeleteEdge } from '../../hooks/useGraphData'
import { Button } from '../ui/Button'
import { Badge } from '../ui/Badge'
import { PropertyList } from './PropertyEditor'
import { getLabelColor } from '../../utils/colors'
import type { GraphNode, GraphEdge, JsonValue } from '../../types/graph'

interface NodeInspectorProps {
  node: GraphNode
  degree: number
  graphName: string
  onDelete: () => void
  onExpand: () => void
}

function NodeInspector({ node, degree, graphName, onDelete, onExpand }: NodeInspectorProps) {
  const [localProps, setLocalProps] = useState<Record<string, JsonValue>>(node.properties)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const updateNode = useUpdateNode(graphName)
  const deleteNode = useDeleteNode(graphName)

  useEffect(() => {
    setLocalProps(node.properties)
    setConfirmDelete(false)
  }, [node.id, node.properties])

  const handlePropChange = (key: string, value: string) => {
    setLocalProps((prev) => ({ ...prev, [key]: value }))
  }

  const handleSave = () => {
    updateNode.mutate({ nodeId: node.id, req: { properties: localProps } })
  }

  const handleDelete = () => {
    if (!confirmDelete) {
      setConfirmDelete(true)
      return
    }
    deleteNode.mutate(node.id, {
      onSuccess: () => onDelete(),
    })
  }

  return (
    <div className="flex flex-col gap-3 p-3">
      {/* ID */}
      <div>
        <p className="text-xs text-gray-500 uppercase tracking-wide mb-1">Node ID</p>
        <p className="text-xs font-mono text-gray-300 break-all">{node.id}</p>
      </div>

      {/* Labels */}
      <div>
        <p className="text-xs text-gray-500 uppercase tracking-wide mb-1">Labels</p>
        <div className="flex flex-wrap gap-1">
          {node.labels.map((label) => (
            <Badge key={label} variant="custom" color={getLabelColor(label)}>
              {label}
            </Badge>
          ))}
        </div>
      </div>

      {/* Degree */}
      <div className="flex items-center gap-2 text-xs text-gray-500">
        <Network size={12} />
        <span>{degree} connection{degree !== 1 ? 's' : ''}</span>
      </div>

      {/* Properties */}
      <div>
        <p className="text-xs text-gray-500 uppercase tracking-wide mb-2">Properties</p>
        <PropertyList nodeId={node.id} properties={localProps} onChange={handlePropChange} />
      </div>

      {/* Actions */}
      <div className="flex flex-col gap-2 pt-2 border-t border-surface-300">
        <Button
          variant="ghost"
          size="sm"
          onClick={onExpand}
          icon={<Network size={14} />}
          className="w-full justify-start text-gray-400"
        >
          Expand Neighborhood
        </Button>
        <Button
          variant="primary"
          size="sm"
          onClick={handleSave}
          loading={updateNode.isPending}
          icon={<Save size={14} />}
          className="w-full justify-center"
        >
          Save Changes
        </Button>
        <Button
          variant="danger"
          size="sm"
          onClick={handleDelete}
          loading={deleteNode.isPending}
          icon={<Trash2 size={14} />}
          className="w-full justify-center"
        >
          {confirmDelete ? 'Confirm Delete?' : 'Delete Node'}
        </Button>
      </div>
    </div>
  )
}

interface EdgeInspectorProps {
  edge: GraphEdge
  graphName: string
  onDelete: () => void
}

function EdgeInspector({ edge, graphName, onDelete }: EdgeInspectorProps) {
  const [localProps, setLocalProps] = useState<Record<string, JsonValue>>(edge.properties)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const updateEdge = useUpdateEdge(graphName)
  const deleteEdge = useDeleteEdge(graphName)

  useEffect(() => {
    setLocalProps(edge.properties)
    setConfirmDelete(false)
  }, [edge.id, edge.properties])

  const handlePropChange = (key: string, value: string) => {
    setLocalProps((prev) => ({ ...prev, [key]: value }))
  }

  const handleSave = () => {
    updateEdge.mutate({ edgeId: edge.id, req: { properties: localProps } })
  }

  const handleDelete = () => {
    if (!confirmDelete) {
      setConfirmDelete(true)
      return
    }
    deleteEdge.mutate(edge.id, {
      onSuccess: () => onDelete(),
    })
  }

  return (
    <div className="flex flex-col gap-3 p-3">
      {/* ID */}
      <div>
        <p className="text-xs text-gray-500 uppercase tracking-wide mb-1">Edge ID</p>
        <p className="text-xs font-mono text-gray-300 break-all">{edge.id}</p>
      </div>

      {/* Type */}
      <div>
        <p className="text-xs text-gray-500 uppercase tracking-wide mb-1">Type</p>
        <Badge variant="accent">{edge.type}</Badge>
      </div>

      {/* Direction */}
      <div className="flex items-center gap-1 text-xs text-gray-400 font-mono">
        <span className="truncate max-w-20">{edge.source.slice(0, 12)}...</span>
        <ChevronRight size={12} className="shrink-0 text-accent-400" />
        <span className="truncate max-w-20">{edge.target.slice(0, 12)}...</span>
      </div>

      {/* Properties */}
      <div>
        <p className="text-xs text-gray-500 uppercase tracking-wide mb-2">Properties</p>
        <PropertyList nodeId={edge.id} properties={localProps} onChange={handlePropChange} />
      </div>

      {/* Actions */}
      <div className="flex flex-col gap-2 pt-2 border-t border-surface-300">
        <Button
          variant="primary"
          size="sm"
          onClick={handleSave}
          loading={updateEdge.isPending}
          icon={<Save size={14} />}
          className="w-full justify-center"
        >
          Save Changes
        </Button>
        <Button
          variant="danger"
          size="sm"
          onClick={handleDelete}
          loading={deleteEdge.isPending}
          icon={<Trash2 size={14} />}
          className="w-full justify-center"
        >
          {confirmDelete ? 'Confirm Delete?' : 'Delete Edge'}
        </Button>
      </div>
    </div>
  )
}

interface InspectorPanelProps {
  selectedNodeData: GraphNode | null
  selectedEdgeData: GraphEdge | null
  nodeDegree: number
  graphName: string | null
  onDeleteNode: (id: string) => void
  onDeleteEdge: (id: string) => void
  onExpandNode: (id: string) => void
}

export function InspectorPanel({
  selectedNodeData,
  selectedEdgeData,
  nodeDegree,
  graphName,
  onDeleteNode,
  onDeleteEdge,
  onExpandNode,
}: InspectorPanelProps) {
  return (
    <div className="h-full overflow-y-auto">
      {/* Header */}
      <div className="px-3 py-2 border-b border-surface-300 sticky top-0 bg-surface-DEFAULT z-10">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wide">
          {selectedNodeData ? 'Node Inspector' : selectedEdgeData ? 'Edge Inspector' : 'Inspector'}
        </h2>
      </div>

      {!selectedNodeData && !selectedEdgeData && (
        <div className="flex items-center justify-center h-32 text-gray-600">
          <p className="text-xs">Select a node or edge</p>
        </div>
      )}

      {selectedNodeData && graphName && (
        <NodeInspector
          node={selectedNodeData}
          degree={nodeDegree}
          graphName={graphName}
          onDelete={() => onDeleteNode(selectedNodeData.id)}
          onExpand={() => onExpandNode(selectedNodeData.id)}
        />
      )}

      {selectedEdgeData && graphName && (
        <EdgeInspector
          edge={selectedEdgeData}
          graphName={graphName}
          onDelete={() => onDeleteEdge(selectedEdgeData.id)}
        />
      )}
    </div>
  )
}
