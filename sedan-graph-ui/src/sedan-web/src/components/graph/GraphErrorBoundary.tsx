import { Component } from 'react'
import type { ErrorInfo, ReactNode } from 'react'

interface ErrorBoundaryState {
  hasError: boolean
  error: string | null
}

/**
 * Error boundary that catches rendering errors in the graph canvas.
 * Displays a retry button instead of crashing the entire app.
 */
export class GraphErrorBoundary extends Component<
  { children: ReactNode },
  ErrorBoundaryState
> {
  constructor(props: { children: ReactNode }) {
    super(props)
    this.state = { hasError: false, error: null }
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error: error.message }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    if (import.meta.env.DEV) {
      console.error('Graph canvas error:', error, info)
    }
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="flex items-center justify-center h-full text-gray-500">
          <div className="text-center">
            <p className="text-base font-medium text-red-400">Graph rendering error</p>
            <p className="text-sm mt-1">{this.state.error}</p>
            <button
              className="mt-3 text-xs text-accent-400 hover:underline"
              onClick={() => this.setState({ hasError: false, error: null })}
            >
              Retry
            </button>
          </div>
        </div>
      )
    }
    return this.props.children
  }
}
