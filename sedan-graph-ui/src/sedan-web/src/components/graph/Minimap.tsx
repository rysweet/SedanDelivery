import { useEffect, useRef } from 'react'
import { useSigma } from '@react-sigma/core'

/**
 * A minimap that renders a scaled-down version of the graph canvas
 * in the bottom-right corner using a canvas element.
 */
export function Minimap() {
  const sigma = useSigma()
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const animRef = useRef<number | null>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return

    const draw = () => {
      const ctx = canvas.getContext('2d')
      if (!ctx) return

      const graph = sigma.getGraph()
      const width = canvas.width
      const height = canvas.height

      ctx.clearRect(0, 0, width, height)
      ctx.fillStyle = '#1e2130'
      ctx.fillRect(0, 0, width, height)

      if (graph.order === 0) return

      // Find bounds
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity
      graph.forEachNode((_id, attrs) => {
        const x = attrs.x as number
        const y = attrs.y as number
        if (x < minX) minX = x
        if (y < minY) minY = y
        if (x > maxX) maxX = x
        if (y > maxY) maxY = y
      })

      const rangeX = maxX - minX || 1
      const rangeY = maxY - minY || 1
      const padding = 8

      const scaleX = (width - padding * 2) / rangeX
      const scaleY = (height - padding * 2) / rangeY
      const scale = Math.min(scaleX, scaleY)

      const toCanvas = (x: number, y: number) => ({
        cx: (x - minX) * scale + padding,
        cy: (y - minY) * scale + padding,
      })

      // Draw edges
      ctx.strokeStyle = '#2d3142'
      ctx.lineWidth = 0.5
      graph.forEachEdge((_id, attrs, source, target) => {
        if (attrs.hidden) return
        const srcAttrs = graph.getNodeAttributes(source) as { x: number; y: number }
        const tgtAttrs = graph.getNodeAttributes(target) as { x: number; y: number }
        const { cx: sx, cy: sy } = toCanvas(srcAttrs.x, srcAttrs.y)
        const { cx: tx, cy: ty } = toCanvas(tgtAttrs.x, tgtAttrs.y)
        ctx.beginPath()
        ctx.moveTo(sx, sy)
        ctx.lineTo(tx, ty)
        ctx.stroke()
      })

      // Draw nodes
      graph.forEachNode((_id, attrs) => {
        if (attrs.hidden) return
        const { cx, cy } = toCanvas(attrs.x as number, attrs.y as number)
        ctx.beginPath()
        ctx.arc(cx, cy, 2, 0, Math.PI * 2)
        ctx.fillStyle = (attrs.color as string) || '#4FC3F7'
        ctx.fill()
      })

      // Draw viewport indicator
      const camera = sigma.getCamera()
      const cameraState = camera.getState()
      const viewRatio = cameraState.ratio

      const domWidth = sigma.getDimensions().width
      const domHeight = sigma.getDimensions().height

      const vpHalfW = (domWidth * viewRatio) / 2
      const vpHalfH = (domHeight * viewRatio) / 2

      const vpMinX = cameraState.x - vpHalfW
      const vpMinY = cameraState.y - vpHalfH
      const vpMaxX = cameraState.x + vpHalfW
      const vpMaxY = cameraState.y + vpHalfH

      const { cx: vl, cy: vt } = toCanvas(vpMinX, vpMinY)
      const { cx: vr, cy: vb } = toCanvas(vpMaxX, vpMaxY)

      ctx.strokeStyle = '#4FC3F7'
      ctx.lineWidth = 1
      ctx.strokeRect(vl, vt, vr - vl, vb - vt)
    }

    const animate = () => {
      draw()
      animRef.current = requestAnimationFrame(animate)
    }
    animRef.current = requestAnimationFrame(animate)

    return () => {
      if (animRef.current) cancelAnimationFrame(animRef.current)
    }
  }, [sigma])

  return (
    <div className="absolute bottom-4 right-4 rounded overflow-hidden border border-surface-300 shadow-lg opacity-80 hover:opacity-100 transition-opacity">
      <canvas
        ref={canvasRef}
        width={160}
        height={100}
        className="block"
        aria-label="Graph minimap"
      />
    </div>
  )
}
