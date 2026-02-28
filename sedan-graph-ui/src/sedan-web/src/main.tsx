import React from 'react'
import ReactDOM from 'react-dom/client'
import { PublicClientApplication, EventType } from '@azure/msal-browser'
import { MsalProvider } from '@azure/msal-react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import App from './App'
import { AuthGuard } from './auth/AuthGuard'
import { msalConfig } from './auth/msalConfig'
import { setMsalInstance } from './api/client'
import './index.css'

const msalInstance = new PublicClientApplication(msalConfig)

// Set the active account after redirect completes
msalInstance.addEventCallback((event) => {
  if (
    event.eventType === EventType.LOGIN_SUCCESS &&
    event.payload &&
    'account' in event.payload &&
    event.payload.account
  ) {
    msalInstance.setActiveAccount(event.payload.account)
  }
})

// Share the MSAL instance with the API client for silent token acquisition
setMsalInstance(msalInstance)

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
      refetchOnWindowFocus: false,
    },
    mutations: {
      retry: 0,
    },
  },
})

const root = document.getElementById('root')
if (!root) throw new Error('Root element not found')

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <MsalProvider instance={msalInstance}>
      <AuthGuard>
        <QueryClientProvider client={queryClient}>
          <App />
        </QueryClientProvider>
      </AuthGuard>
    </MsalProvider>
  </React.StrictMode>,
)
