import { type Configuration, LogLevel } from '@azure/msal-browser'

export const msalConfig: Configuration = {
  auth: {
    clientId: import.meta.env.VITE_ENTRA_CLIENT_ID ?? '97926b31-e1a3-4a76-b09d-eed5aeeabc9f',
    authority: `https://login.microsoftonline.com/${import.meta.env.VITE_ENTRA_TENANT_ID ?? '3cd87a41-1f61-4aef-a212-cefdecd9a2d1'}`,
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
  scopes: [
    import.meta.env.VITE_ENTRA_API_SCOPE ?? 'api://97926b31-e1a3-4a76-b09d-eed5aeeabc9f/Graph.ReadWrite',
  ],
}

export const apiTokenRequest = {
  scopes: [
    import.meta.env.VITE_ENTRA_API_SCOPE ?? 'api://97926b31-e1a3-4a76-b09d-eed5aeeabc9f/Graph.ReadWrite',
  ],
}
