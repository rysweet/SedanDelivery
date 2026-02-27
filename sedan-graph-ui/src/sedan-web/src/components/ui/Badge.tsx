import { clsx } from 'clsx'
import { twMerge } from 'tailwind-merge'
import { X } from 'lucide-react'

type BadgeVariant = 'default' | 'accent' | 'success' | 'warning' | 'danger' | 'custom'

interface BadgeProps {
  children: React.ReactNode
  variant?: BadgeVariant
  color?: string
  onRemove?: () => void
  className?: string
}

const variantClasses: Record<BadgeVariant, string> = {
  default: 'bg-surface-200 text-gray-300 border border-surface-300',
  accent: 'bg-accent-400/20 text-accent-400 border border-accent-400/30',
  success: 'bg-green-900/30 text-green-400 border border-green-700/30',
  warning: 'bg-yellow-900/30 text-yellow-400 border border-yellow-700/30',
  danger: 'bg-red-900/30 text-red-400 border border-red-700/30',
  custom: '',
}

export function Badge({
  children,
  variant = 'default',
  color,
  onRemove,
  className,
}: BadgeProps) {
  const style =
    variant === 'custom' && color
      ? {
          backgroundColor: `${color}20`,
          color,
          borderColor: `${color}40`,
        }
      : undefined

  return (
    <span
      className={twMerge(
        clsx(
          'inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium',
          variantClasses[variant],
          className,
        ),
      )}
      style={style}
    >
      {children}
      {onRemove && (
        <button
          onClick={onRemove}
          className="hover:opacity-70 transition-opacity"
          aria-label="Remove"
        >
          <X size={10} />
        </button>
      )}
    </span>
  )
}
