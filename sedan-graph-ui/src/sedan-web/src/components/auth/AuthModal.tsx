import { useState, useCallback } from 'react'
import { Lock } from 'lucide-react'
import { useAuthStore } from '../../store/auth'
import { Button } from '../ui/Button'

export function AuthModal() {
  const requiresAuth = useAuthStore((s) => s.requiresAuth)
  const setToken = useAuthStore((s) => s.setToken)
  const [inputValue, setInputValue] = useState('')
  const [error, setError] = useState<string | null>(null)

  const handleSubmit = useCallback(
    (e: React.FormEvent) => {
      e.preventDefault()
      const trimmed = inputValue.trim()
      if (!trimmed) {
        setError('API key cannot be empty')
        return
      }
      setError(null)
      setToken(trimmed)
      setInputValue('')
    },
    [inputValue, setToken],
  )

  if (!requiresAuth) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby="auth-modal-title"
    >
      {/* Backdrop - non-dismissible */}
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" aria-hidden="true" />

      {/* Panel */}
      <div className="relative w-full max-w-sm rounded-lg bg-surface-100 border border-surface-300 shadow-2xl shadow-black/50">
        <div className="p-6">
          <div className="flex items-center gap-2 mb-1">
            <Lock size={18} className="text-accent-400" />
            <h2 id="auth-modal-title" className="text-base font-semibold text-gray-100">
              Authentication Required
            </h2>
          </div>
          <p className="text-sm text-gray-400 mb-4">Enter your API key to connect</p>

          <form onSubmit={handleSubmit} className="space-y-3">
            <input
              type="password"
              placeholder="API key"
              value={inputValue}
              onChange={(e) => { setInputValue(e.target.value); setError(null) }}
              autoFocus
              className="w-full px-3 py-2 text-sm rounded bg-surface-200 border border-surface-300 text-gray-200 placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-accent-400 focus:border-transparent"
            />
            {error && <p className="text-xs text-red-400">{error}</p>}
            <Button type="submit" variant="primary" size="sm" className="w-full">
              Connect
            </Button>
          </form>
        </div>
      </div>
    </div>
  )
}
