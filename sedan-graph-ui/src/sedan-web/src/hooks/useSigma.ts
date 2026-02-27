import { useEffect, useRef, useCallback } from 'react'
import { useSigma as useSigmaCore } from '@react-sigma/core'
import { useGraphStore } from '../store/graph'

/**
 * Hook that registers Sigma.js event handlers for node/edge interactions.
 */
export function useSigmaEvents() {
  const sigma = useSigmaCore()
  const setSelectedNode = useGraphStore((s) => s.setSelectedNode)
  const setSelectedEdge = useGraphStore((s) => s.setSelectedEdge)
  const setHoveredNode = useGraphStore((s) => s.setHoveredNode)
  const clearSelection = useGraphStore((s) => s.clearSelection)
  const activeMode = useGraphStore((s) => s.activeMode)
  const setZoomLevel = useGraphStore((s) => s.setZoomLevel)

  const hoverTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    const camera = sigma.getCamera()

    const handleCameraUpdate = () => {
      const ratio = camera.ratio
      setZoomLevel(Math.round((1 / ratio) * 100) / 100)
    }

    camera.on('updated', handleCameraUpdate)
    return () => {
      camera.removeListener('updated', handleCameraUpdate)
    }
  }, [sigma, setZoomLevel])

  useEffect(() => {
    const handleNodeClick = (event: { node: string }) => {
      if (activeMode === 'select') {
        setSelectedNode(event.node)
      }
    }

    const handleEdgeClick = (event: { edge: string }) => {
      if (activeMode === 'select') {
        setSelectedEdge(event.edge)
      }
    }

    const handleStageClick = () => {
      clearSelection()
    }

    const handleNodeEnter = (event: { node: string }) => {
      if (hoverTimerRef.current) {
        clearTimeout(hoverTimerRef.current)
      }
      hoverTimerRef.current = setTimeout(() => {
        setHoveredNode(event.node)
      }, 200)
    }

    const handleNodeLeave = () => {
      if (hoverTimerRef.current) {
        clearTimeout(hoverTimerRef.current)
        hoverTimerRef.current = null
      }
      setHoveredNode(null)
    }

    sigma.on('clickNode', handleNodeClick)
    sigma.on('clickEdge', handleEdgeClick)
    sigma.on('clickStage', handleStageClick)
    sigma.on('enterNode', handleNodeEnter)
    sigma.on('leaveNode', handleNodeLeave)

    return () => {
      sigma.off('clickNode', handleNodeClick)
      sigma.off('clickEdge', handleEdgeClick)
      sigma.off('clickStage', handleStageClick)
      sigma.off('enterNode', handleNodeEnter)
      sigma.off('leaveNode', handleNodeLeave)
      if (hoverTimerRef.current) {
        clearTimeout(hoverTimerRef.current)
      }
    }
  }, [sigma, activeMode, setSelectedNode, setSelectedEdge, setHoveredNode, clearSelection])
}

/**
 * Hook that provides camera control utilities.
 */
export function useSigmaCamera() {
  const sigma = useSigmaCore()

  const fitView = useCallback(() => {
    sigma.getCamera().animatedReset()
  }, [sigma])

  const zoomIn = useCallback(() => {
    sigma.getCamera().animatedZoom({ duration: 300 })
  }, [sigma])

  const zoomOut = useCallback(() => {
    sigma.getCamera().animatedUnzoom({ duration: 300 })
  }, [sigma])

  const focusNode = useCallback(
    (nodeId: string) => {
      const graph = sigma.getGraph()
      if (!graph.hasNode(nodeId)) return

      const nodeAttrs = graph.getNodeAttributes(nodeId)
      const pos = sigma.graphToViewport({ x: nodeAttrs.x as number, y: nodeAttrs.y as number })
      if (!pos) return

      sigma.getCamera().animate(
        { x: nodeAttrs.x as number, y: nodeAttrs.y as number, ratio: 0.5 },
        { duration: 500 },
      )
    },
    [sigma],
  )

  return { fitView, zoomIn, zoomOut, focusNode }
}
