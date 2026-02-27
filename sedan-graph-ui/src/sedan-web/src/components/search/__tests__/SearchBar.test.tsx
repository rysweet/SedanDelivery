import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { SearchBar } from '../SearchBar'
import { useGraphStore } from '../../../store/graph'

// Mock the API client
vi.mock('../../../api/client', () => ({
  queryApi: {
    query: vi.fn(),
  },
}))

function wrapper({ children }: { children: React.ReactNode }) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
}

describe('SearchBar', () => {
  beforeEach(() => {
    useGraphStore.setState({ currentGraph: 'testGraph' })
    vi.clearAllMocks()
  })

  it('renders the search input', () => {
    const onResultsFound = vi.fn()
    render(<SearchBar onResultsFound={onResultsFound} />, { wrapper })
    expect(screen.getByPlaceholderText(/search by label or property/i)).toBeInTheDocument()
  })

  it('input is disabled when no graph is selected', () => {
    useGraphStore.setState({ currentGraph: null })
    const onResultsFound = vi.fn()
    render(<SearchBar onResultsFound={onResultsFound} />, { wrapper })
    const input = screen.getByLabelText(/search graph/i)
    expect(input).toBeDisabled()
  })

  it('input is enabled when a graph is selected', () => {
    const onResultsFound = vi.fn()
    render(<SearchBar onResultsFound={onResultsFound} />, { wrapper })
    const input = screen.getByLabelText(/search graph/i)
    expect(input).not.toBeDisabled()
  })

  it('updates input value on type', async () => {
    const user = userEvent.setup()
    const onResultsFound = vi.fn()
    render(<SearchBar onResultsFound={onResultsFound} />, { wrapper })
    const input = screen.getByLabelText(/search graph/i)
    await user.type(input, 'Alice')
    expect(input).toHaveValue('Alice')
  })

  it('shows clear button when input has value', async () => {
    const user = userEvent.setup()
    const onResultsFound = vi.fn()
    render(<SearchBar onResultsFound={onResultsFound} />, { wrapper })
    const input = screen.getByLabelText(/search graph/i)
    await user.type(input, 'test')
    expect(screen.getByLabelText(/clear search/i)).toBeInTheDocument()
  })

  it('clears input when clear button is clicked', async () => {
    const user = userEvent.setup()
    const onResultsFound = vi.fn()
    render(<SearchBar onResultsFound={onResultsFound} />, { wrapper })
    const input = screen.getByLabelText(/search graph/i)
    await user.type(input, 'Alice')
    await user.click(screen.getByLabelText(/clear search/i))
    expect(input).toHaveValue('')
  })

  it('does not show clear button when input is empty', () => {
    const onResultsFound = vi.fn()
    render(<SearchBar onResultsFound={onResultsFound} />, { wrapper })
    expect(screen.queryByLabelText(/clear search/i)).not.toBeInTheDocument()
  })

  it('has correct aria label', () => {
    const onResultsFound = vi.fn()
    render(<SearchBar onResultsFound={onResultsFound} />, { wrapper })
    expect(screen.getByRole('textbox', { name: /search graph/i })).toBeInTheDocument()
  })
})
