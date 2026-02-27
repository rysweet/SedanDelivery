import { forwardRef } from 'react'
import { clsx } from 'clsx'
import { twMerge } from 'tailwind-merge'

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'outline'
type ButtonSize = 'xs' | 'sm' | 'md' | 'lg'

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant
  size?: ButtonSize
  loading?: boolean
  icon?: React.ReactNode
  iconPosition?: 'left' | 'right'
}

const variantClasses: Record<ButtonVariant, string> = {
  primary:
    'bg-accent-400 text-surface-DEFAULT hover:bg-accent-500 focus:ring-accent-400 font-medium',
  secondary:
    'bg-surface-200 text-gray-200 hover:bg-surface-300 focus:ring-surface-300 border border-surface-300',
  ghost:
    'bg-transparent text-gray-300 hover:bg-surface-100 hover:text-white focus:ring-surface-200',
  danger:
    'bg-red-600 text-white hover:bg-red-700 focus:ring-red-500 font-medium',
  outline:
    'bg-transparent text-accent-400 border border-accent-400 hover:bg-accent-400 hover:text-surface-DEFAULT focus:ring-accent-400',
}

const sizeClasses: Record<ButtonSize, string> = {
  xs: 'px-2 py-1 text-xs gap-1',
  sm: 'px-3 py-1.5 text-sm gap-1.5',
  md: 'px-4 py-2 text-sm gap-2',
  lg: 'px-5 py-2.5 text-base gap-2',
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  (
    {
      variant = 'secondary',
      size = 'md',
      loading = false,
      icon,
      iconPosition = 'left',
      children,
      className,
      disabled,
      ...props
    },
    ref,
  ) => {
    const isDisabled = disabled ?? loading

    return (
      <button
        ref={ref}
        disabled={isDisabled}
        className={twMerge(
          clsx(
            'inline-flex items-center justify-center rounded transition-colors',
            'focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-offset-surface-DEFAULT',
            'disabled:opacity-50 disabled:cursor-not-allowed',
            variantClasses[variant],
            sizeClasses[size],
            className,
          ),
        )}
        {...props}
      >
        {loading && (
          <svg
            className="animate-spin h-4 w-4"
            xmlns="http://www.w3.org/2000/svg"
            fill="none"
            viewBox="0 0 24 24"
            aria-hidden="true"
          >
            <circle
              className="opacity-25"
              cx="12"
              cy="12"
              r="10"
              stroke="currentColor"
              strokeWidth="4"
            />
            <path
              className="opacity-75"
              fill="currentColor"
              d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
            />
          </svg>
        )}
        {!loading && icon && iconPosition === 'left' && (
          <span className="shrink-0">{icon}</span>
        )}
        {children && <span>{children}</span>}
        {!loading && icon && iconPosition === 'right' && (
          <span className="shrink-0">{icon}</span>
        )}
      </button>
    )
  },
)

Button.displayName = 'Button'
