import { useIsAuthenticated, useMsal } from '@azure/msal-react'
import { InteractionStatus } from '@azure/msal-browser'
import { loginRequest } from './msalConfig'

const isTestAuth = import.meta.env.VITE_E2E_TEST_AUTH === 'true'

function LoadingSpinner({ message = 'Signing in...' }: { message?: string }) {
  return (
    <div className="flex items-center justify-center h-screen bg-surface-50">
      <div className="flex flex-col items-center gap-3">
        <div className="h-8 w-8 animate-spin rounded-full border-2 border-accent-400 border-t-transparent" />
        <span className="text-sm text-gray-400">{message}</span>
      </div>
    </div>
  )
}

export function AuthGuard({ children }: { children: React.ReactNode }) {
  const isAuthenticated = useIsAuthenticated()
  const { instance, inProgress } = useMsal()

  // In E2E test mode, skip MSAL sign-in flow — the API client sends X-Test-Auth header
  if (isTestAuth) return <>{children}</>

  if (inProgress !== InteractionStatus.None) {
    return <LoadingSpinner />
  }

  if (!isAuthenticated) {
    instance.loginRedirect(loginRequest).catch(console.error)
    return <LoadingSpinner message="Redirecting to sign in..." />
  }

  return <>{children}</>
}
