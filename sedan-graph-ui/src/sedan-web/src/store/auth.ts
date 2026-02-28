import { create } from 'zustand'

const SESSION_KEY = 'sedan_api_token'

interface AuthState {
  token: string | null
  requiresAuth: boolean
  setToken: (token: string) => void
  clearToken: () => void
  setRequiresAuth: (value: boolean) => void
}

export const useAuthStore = create<AuthState>((set) => ({
  token: sessionStorage.getItem(SESSION_KEY),
  requiresAuth: false,
  setToken: (token) => {
    sessionStorage.setItem(SESSION_KEY, token)
    set({ token, requiresAuth: false })
  },
  clearToken: () => {
    sessionStorage.removeItem(SESSION_KEY)
    set({ token: null, requiresAuth: false })
  },
  setRequiresAuth: (value) => set({ requiresAuth: value }),
}))
