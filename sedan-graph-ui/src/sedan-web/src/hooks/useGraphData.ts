import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  graphsApi,
  graphDataApi,
  nodesApi,
  edgesApi,
  queryApi,
} from '../api/client'
import type {
  CreateNodeRequest,
  UpdateNodeRequest,
  CreateEdgeRequest,
  UpdateEdgeRequest,
  QueryRequest,
  MutateRequest,
  CreateGraphRequest,
} from '../types/api'

// Query keys
export const queryKeys = {
  graphs: ['graphs'] as const,
  graphData: (name: string) => ['graphs', name, 'data'] as const,
  graphSchema: (name: string) => ['graphs', name, 'schema'] as const,
  neighbors: (graphName: string, nodeId: string) =>
    ['graphs', graphName, 'neighbors', nodeId] as const,
}

// Graph list
export function useGraphList() {
  return useQuery({
    queryKey: queryKeys.graphs,
    queryFn: () => graphsApi.list(),
  })
}

// Graph data
export function useGraphData(graphName: string | null) {
  return useQuery({
    queryKey: queryKeys.graphData(graphName ?? ''),
    queryFn: () => graphDataApi.getAll(graphName!),
    enabled: graphName !== null,
  })
}

// Graph schema
export function useGraphSchema(graphName: string | null) {
  return useQuery({
    queryKey: queryKeys.graphSchema(graphName ?? ''),
    queryFn: () => graphDataApi.getSchema(graphName!),
    enabled: graphName !== null,
  })
}

// Node neighbors (manual fetch, not auto-fetched)
export function useNeighbors(graphName: string | null, nodeId: string | null) {
  return useQuery({
    queryKey: queryKeys.neighbors(graphName ?? '', nodeId ?? ''),
    queryFn: () => graphDataApi.getNeighbors(graphName!, nodeId!),
    enabled: false, // Triggered manually
  })
}

// Mutations
export function useCreateGraph() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: CreateGraphRequest) => graphsApi.create(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.graphs })
    },
  })
}

export function useDeleteGraph() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (name: string) => graphsApi.delete(name),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.graphs })
    },
  })
}

export function useCreateNode(graphName: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: CreateNodeRequest) => nodesApi.create(graphName!, req),
    onSuccess: () => {
      if (graphName) {
        queryClient.invalidateQueries({ queryKey: queryKeys.graphData(graphName) })
        queryClient.invalidateQueries({ queryKey: queryKeys.graphSchema(graphName) })
      }
    },
  })
}

export function useUpdateNode(graphName: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ nodeId, req }: { nodeId: string; req: UpdateNodeRequest }) =>
      nodesApi.update(graphName!, nodeId, req),
    onSuccess: () => {
      if (graphName) {
        queryClient.invalidateQueries({ queryKey: queryKeys.graphData(graphName) })
      }
    },
  })
}

export function useDeleteNode(graphName: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (nodeId: string) => nodesApi.delete(graphName!, nodeId),
    onSuccess: () => {
      if (graphName) {
        queryClient.invalidateQueries({ queryKey: queryKeys.graphData(graphName) })
        queryClient.invalidateQueries({ queryKey: queryKeys.graphSchema(graphName) })
      }
    },
  })
}

export function useCreateEdge(graphName: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: CreateEdgeRequest) => edgesApi.create(graphName!, req),
    onSuccess: () => {
      if (graphName) {
        queryClient.invalidateQueries({ queryKey: queryKeys.graphData(graphName) })
        queryClient.invalidateQueries({ queryKey: queryKeys.graphSchema(graphName) })
      }
    },
  })
}

export function useUpdateEdge(graphName: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ edgeId, req }: { edgeId: string; req: UpdateEdgeRequest }) =>
      edgesApi.update(graphName!, edgeId, req),
    onSuccess: () => {
      if (graphName) {
        queryClient.invalidateQueries({ queryKey: queryKeys.graphData(graphName) })
      }
    },
  })
}

export function useDeleteEdge(graphName: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (edgeId: string) => edgesApi.delete(graphName!, edgeId),
    onSuccess: () => {
      if (graphName) {
        queryClient.invalidateQueries({ queryKey: queryKeys.graphData(graphName) })
        queryClient.invalidateQueries({ queryKey: queryKeys.graphSchema(graphName) })
      }
    },
  })
}

export function useQuery_(graphName: string | null) {
  return useMutation({
    mutationFn: (req: QueryRequest) => queryApi.query(graphName!, req),
  })
}

export function useMutate(graphName: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: MutateRequest) => queryApi.mutate(graphName!, req),
    onSuccess: () => {
      if (graphName) {
        queryClient.invalidateQueries({ queryKey: queryKeys.graphData(graphName) })
        queryClient.invalidateQueries({ queryKey: queryKeys.graphSchema(graphName) })
      }
    },
  })
}
