import { type Configuration, LogLevel } from '@azure/msal-browser'

function requireEnv(name: string): string {
  const value = import.meta.env[name]
  if (!value) {
    throw new Error(
      `${name} is required. Add it to .env.development (see .env.development.example).`,
    )
  }
  return value
}

const clientId = requireEnv('VITE_ENTRA_CLIENT_ID')
const tenantId = requireEnv('VITE_ENTRA_TENANT_ID')
const apiScope = requireEnv('VITE_ENTRA_API_SCOPE')

export const msalConfig: Configuration = {
  auth: {
    clientId,
    authority: `https://login.microsoftonline.com/${tenantId}`,
    redirectUri: window.location.origin,
    postLogoutRedirectUri: window.location.origin,
  },
  cache: {
    cacheLocation: 'sessionStorage',
    storeAuthStateInCookie: false,
  },
  system: {
    loggerOptions: {
      logLevel: import.meta.env.DEV ? LogLevel.Warning : LogLevel.Error,
    },
  },
}

export const loginRequest = {
  scopes: [apiScope],
}

export const apiTokenRequest = {
  scopes: [apiScope],
}
