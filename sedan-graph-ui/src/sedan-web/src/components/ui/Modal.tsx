import { useEffect, useCallback } from 'react'
import { X } from 'lucide-react'
import { clsx } from 'clsx'
import { Button } from './Button'

interface ModalProps {
  open: boolean
  onClose: () => void
  title: string
  description?: string
  children: React.ReactNode
  footer?: React.ReactNode
  size?: 'sm' | 'md' | 'lg' | 'xl'
}

const sizeClasses = {
  sm: 'max-w-sm',
  md: 'max-w-md',
  lg: 'max-w-lg',
  xl: 'max-w-2xl',
}

export function Modal({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  size = 'md',
}: ModalProps) {
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    },
    [onClose],
  )

  useEffect(() => {
    if (open) {
      document.addEventListener('keydown', handleKeyDown)
      document.body.style.overflow = 'hidden'
    }
    return () => {
      document.removeEventListener('keydown', handleKeyDown)
      document.body.style.overflow = ''
    }
  }, [open, handleKeyDown])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby="modal-title"
    >
      {/* Backdrop */}
      <div
        className="absolute inset-0 bg-black/60 backdrop-blur-sm"
        onClick={onClose}
        aria-hidden="true"
      />

      {/* Panel */}
      <div
        className={clsx(
          'relative w-full rounded-lg bg-surface-100 border border-surface-300',
          'shadow-2xl shadow-black/50',
          sizeClasses[size],
        )}
      >
        {/* Header */}
        <div className="flex items-start justify-between p-4 border-b border-surface-300">
          <div>
            <h2
              id="modal-title"
              className="text-base font-semibold text-gray-100"
            >
              {title}
            </h2>
            {description && (
              <p className="mt-1 text-sm text-gray-400">{description}</p>
            )}
          </div>
          <Button
            variant="ghost"
            size="xs"
            onClick={onClose}
            className="ml-4 shrink-0"
            aria-label="Close dialog"
          >
            <X size={16} />
          </Button>
        </div>

        {/* Body */}
        <div className="p-4">{children}</div>

        {/* Footer */}
        {footer && (
          <div className="flex items-center justify-end gap-2 p-4 border-t border-surface-300">
            {footer}
          </div>
        )}
      </div>
    </div>
  )
}
