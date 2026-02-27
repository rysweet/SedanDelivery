import { useEffect, useRef, useCallback } from 'react'
import { useSigma } from '@react-sigma/core'
import { circular } from 'graphology-layout'
import FA2Layout from 'graphology-layout-forceatlas2/worker'
import { random } from 'graphology-layout'
import { useGraphStore } from '../store/graph'
import type { LayoutAlgorithm } from '../types/graph'

/**
 * Hook that manages layout algorithms for the graph.
 * Supports forceAtlas2 (via web worker), circular, and random.
 */
export function useLayout() {
  const sigma = useSigma()
  const fa2WorkerRef = useRef<FA2Layout | null>(null)
  const currentLayout = useGraphStore((s) => s.currentLayout)
  const setLayoutRunning = useGraphStore((s) => s.setLayoutRunning)
  const layoutRunning = useGraphStore((s) => s.layoutRunning)

  const stopFA2 = useCallback(() => {
    if (fa2WorkerRef.current) {
      fa2WorkerRef.current.stop()
      fa2WorkerRef.current.kill()
      fa2WorkerRef.current = null
      setLayoutRunning(false)
    }
  }, [setLayoutRunning])

  const applyLayout = useCallback(
    (algorithm: LayoutAlgorithm) => {
      const graph = sigma.getGraph()
      if (graph.order === 0) return

      // Stop any running FA2
      stopFA2()

      switch (algorithm) {
        case 'forceAtlas2': {
          const worker = new FA2Layout(graph, {
            settings: {
              gravity: 1,
              scalingRatio: 2,
              slowDown: 3,
              barnesHutOptimize: graph.order > 500,
            },
          })
          worker.start()
          fa2WorkerRef.current = worker
          setLayoutRunning(true)

          // Auto-stop after 5 seconds
          setTimeout(() => {
            if (fa2WorkerRef.current === worker) {
              stopFA2()
            }
          }, 5000)
          break
        }

        case 'circular':
          circular.assign(graph)
          sigma.refresh()
          break

        case 'random':
          random.assign(graph)
          sigma.refresh()
          break
      }
    },
    [sigma, stopFA2, setLayoutRunning],
  )

  // Apply layout when currentLayout changes
  useEffect(() => {
    applyLayout(currentLayout)
  }, [currentLayout, applyLayout])

  // Clean up on unmount
  useEffect(() => {
    return () => {
      stopFA2()
    }
  }, [stopFA2])

  return {
    applyLayout,
    stopLayout: stopFA2,
    layoutRunning,
  }
}
