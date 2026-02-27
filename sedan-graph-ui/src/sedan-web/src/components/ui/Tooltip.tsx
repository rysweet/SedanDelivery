import { useState, useRef, useCallback } from 'react'
import { clsx } from 'clsx'

type TooltipPlacement = 'top' | 'bottom' | 'left' | 'right'

interface TooltipProps {
  content: React.ReactNode
  placement?: TooltipPlacement
  delay?: number
  children: React.ReactElement
  disabled?: boolean
}

const placementClasses: Record<TooltipPlacement, string> = {
  top: 'bottom-full left-1/2 -translate-x-1/2 mb-2',
  bottom: 'top-full left-1/2 -translate-x-1/2 mt-2',
  left: 'right-full top-1/2 -translate-y-1/2 mr-2',
  right: 'left-full top-1/2 -translate-y-1/2 ml-2',
}

export function Tooltip({
  content,
  placement = 'top',
  delay = 400,
  children,
  disabled = false,
}: TooltipProps) {
  const [visible, setVisible] = useState(false)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const show = useCallback(() => {
    if (disabled) return
    timerRef.current = setTimeout(() => setVisible(true), delay)
  }, [disabled, delay])

  const hide = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current)
      timerRef.current = null
    }
    setVisible(false)
  }, [])

  return (
    <div className="relative inline-flex" onMouseEnter={show} onMouseLeave={hide}>
      {children}
      {visible && (
        <div
          role="tooltip"
          className={clsx(
            'absolute z-50 px-2 py-1 rounded text-xs text-white',
            'bg-gray-900 border border-surface-300 shadow-lg whitespace-nowrap',
            'pointer-events-none',
            placementClasses[placement],
          )}
        >
          {content}
        </div>
      )}
    </div>
  )
}
