import { useEffect } from 'react'

interface ToastProps {
  message: string
  type: 'error' | 'success' | 'info'
  onDismiss: () => void
  durationMs?: number
}

export function Toast({ message, type, onDismiss, durationMs = 5000 }: ToastProps) {
  useEffect(() => {
    const timer = setTimeout(onDismiss, durationMs)
    return () => clearTimeout(timer)
  }, [onDismiss, durationMs])

  const colors = {
    error: 'bg-red-900 border-red-700 text-red-100',
    success: 'bg-green-900 border-green-700 text-green-100',
    info: 'bg-blue-900 border-blue-700 text-blue-100',
  }

  return (
    <div
      role="alert"
      aria-live="assertive"
      className={`fixed bottom-4 right-4 z-50 max-w-sm rounded-lg border px-4 py-3 shadow-lg ${colors[type]}`}
    >
      <div className="flex items-start gap-2">
        <span className="flex-1 text-sm">{message}</span>
        <button
          onClick={onDismiss}
          aria-label="Dismiss notification"
          className="text-current opacity-70 hover:opacity-100"
        >
          ×
        </button>
      </div>
    </div>
  )
}
