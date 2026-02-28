import { useQuery } from '@tanstack/react-query'
import { CheckCircle, XCircle, Clock, Database, Server, Activity, ArrowLeft } from 'lucide-react'

interface HealthResponse { status: string; version: string; demoGraphExists: boolean }
interface GraphStats { nodeCount: number; edgeCount: number; labelCount: number; typeCount: number }
interface GraphListResponse { graphs: string[] }
interface GraphDetail { name: string; stats: GraphStats | null; latencyMs: number }

const BASE_URL = import.meta.env.VITE_API_URL ?? ''

async function fetchWithLatency<T>(path: string): Promise<{ data: T; latencyMs: number }> {
  const start = performance.now()
  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  const res = await fetch(`${BASE_URL}${path}`, { headers })
  const latencyMs = Math.round(performance.now() - start)
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return { data: (await res.json()) as T, latencyMs }
}

const LATENCY_GOOD_MS = 100
const LATENCY_WARN_MS = 500
const HEALTH_REFETCH_MS = 30_000

function latencyColor(ms: number): string {
  if (ms < LATENCY_GOOD_MS) return 'text-green-400'
  if (ms < LATENCY_WARN_MS) return 'text-yellow-400'
  return 'text-red-400'
}

function LatencyBadge({ ms }: { ms: number }) {
  return (
    <span className={`inline-flex items-center gap-1 text-xs font-mono ${latencyColor(ms)}`}>
      <Clock size={12} />{ms}ms
    </span>
  )
}

export function StatusPage() {
  const health = useQuery({
    queryKey: ['status', 'health'],
    queryFn: () => fetchWithLatency<HealthResponse>('/api/health'),
    refetchInterval: HEALTH_REFETCH_MS,
  })

  const graphs = useQuery({
    queryKey: ['status', 'graphs'],
    queryFn: async () => {
      const list = await fetchWithLatency<GraphListResponse>('/api/graphs')
      const details: GraphDetail[] = []
      for (const name of list.data.graphs) {
        try {
          const s = await fetchWithLatency<GraphStats>(`/api/graphs/${encodeURIComponent(name)}/stats`)
          details.push({ name, stats: s.data, latencyMs: s.latencyMs })
        } catch {
          details.push({ name, stats: null, latencyMs: 0 })
        }
      }
      return { listLatencyMs: list.latencyMs, details }
    },
    refetchInterval: HEALTH_REFETCH_MS,
  })

  const isHealthy = health.data && health.data.data.status === 'ok'
  const overallOk = isHealthy && !health.isError && !graphs.isError

  return (
    <div className="min-h-screen bg-surface-50 text-gray-200 p-6">
      <div className="max-w-3xl mx-auto space-y-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-bold text-gray-100 flex items-center gap-2">
            <Activity size={20} className="text-accent-400" />
            SedanDelivery Status
          </h1>
          <div className="flex items-center gap-3">
            <span className="text-xs text-gray-500">Auto-refresh: 30s</span>
            <button
              onClick={() => { health.refetch(); graphs.refetch() }}
              className="px-3 py-1.5 text-xs rounded bg-surface-200 text-gray-300 hover:bg-surface-300 transition-colors"
            >Refresh</button>
            <button
              onClick={() => { window.location.hash = '' }}
              className="flex items-center gap-1 px-3 py-1.5 text-xs rounded bg-surface-200 text-gray-300 hover:bg-surface-300 transition-colors"
            ><ArrowLeft size={12} />Back to Graph Explorer</button>
          </div>
        </div>

        <div className={`rounded-lg px-4 py-3 flex items-center gap-2 text-sm font-medium ${
          overallOk
            ? 'bg-green-900/40 border border-green-700 text-green-300'
            : 'bg-red-900/40 border border-red-700 text-red-300'
        }`}>
          {overallOk ? <CheckCircle size={16} /> : <XCircle size={16} />}
          {overallOk ? 'All systems operational' : 'Service degradation detected'}
        </div>

        <div className="rounded-lg bg-surface-100 border border-surface-300 p-4 space-y-3">
          <h2 className="text-sm font-semibold text-gray-300 flex items-center gap-2">
            <Server size={14} />Service Health
          </h2>
          {health.isLoading && <p className="text-xs text-gray-500">Checking...</p>}
          {health.isError && <p className="text-xs text-red-400">Failed to reach API server</p>}
          {health.data && (
            <div className="grid grid-cols-3 gap-4 text-xs">
              <div>
                <span className="text-gray-500">Status</span>
                <div className={isHealthy ? 'text-green-400' : 'text-red-400'}>{health.data.data.status}</div>
              </div>
              <div>
                <span className="text-gray-500">Version</span>
                <div className="text-gray-300">{health.data.data.version}</div>
              </div>
              <div>
                <span className="text-gray-500">Latency</span>
                <LatencyBadge ms={health.data.latencyMs} />
              </div>
            </div>
          )}
        </div>

        <div className="rounded-lg bg-surface-100 border border-surface-300 p-4 space-y-3">
          <h2 className="text-sm font-semibold text-gray-300 flex items-center gap-2">
            <Database size={14} />Graphs
          </h2>
          {graphs.isLoading && <p className="text-xs text-gray-500">Loading graphs...</p>}
          {graphs.isError && <p className="text-xs text-red-400">Failed to load graph list</p>}
          {graphs.data && graphs.data.details.length === 0 && (
            <p className="text-xs text-gray-500">No graphs found</p>
          )}
          {graphs.data && graphs.data.details.map((g) => (
            <div key={g.name} className="flex items-center justify-between py-2 px-3 rounded bg-surface-200/50 text-xs">
              <div className="flex items-center gap-2">
                <Database size={12} className="text-accent-400" />
                <span className="text-gray-200 font-medium">{g.name}</span>
              </div>
              <div className="flex items-center gap-4">
                {g.stats ? (
                  <>
                    <span className="text-gray-400">{g.stats.nodeCount} nodes</span>
                    <span className="text-gray-400">{g.stats.edgeCount} edges</span>
                  </>
                ) : <span className="text-gray-500">stats unavailable</span>}
                <LatencyBadge ms={g.latencyMs} />
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
