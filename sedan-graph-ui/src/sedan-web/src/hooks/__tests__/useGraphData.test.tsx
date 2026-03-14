import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { graphsApi } from '../../api/client'
import { queryKeys, useDeleteGraph } from '../useGraphData'
import { useGraphStore } from '../../store/graph'

vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<typeof import('../../api/client')>('../../api/client')
  return {
    ...actual,
    graphsApi: {
      ...actual.graphsApi,
      delete: vi.fn(),
    },
  }
})

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  })
  return {
    queryClient,
    wrapper({ children }: { children: ReactNode }) {
      return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    },
  }
}

describe('useDeleteGraph', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useGraphStore.setState({ toast: null })
  })

  it('surfaces delete failures through the toast store', async () => {
    vi.mocked(graphsApi.delete).mockRejectedValue(
      new Error("graph 'bigdb' is too large for atomic deletion; use selective deletion instead"),
    )

    const { queryClient, wrapper } = createWrapper()
    const invalidateQueries = vi.spyOn(queryClient, 'invalidateQueries')
    const { result } = renderHook(() => useDeleteGraph(), { wrapper })

    await expect(result.current.mutateAsync('bigdb')).rejects.toThrow(
      "graph 'bigdb' is too large for atomic deletion; use selective deletion instead",
    )

    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: queryKeys.graphs })

    expect(useGraphStore.getState().toast).toEqual({
      message:
        "Failed to delete graph: graph 'bigdb' is too large for atomic deletion; use selective deletion instead",
      type: 'error',
    })
  })
})
