export interface Service {
  name: string
  ty: string
  local_addr: string
  remote_port: number
  host?: string
  subdomain?: string
  healthy: boolean
  enabled: boolean
  current_conns: number
  traffic_tx: number
  traffic_rx: number
  client_id: string
  uptime_secs: number
  group?: string
  bandwidthLimit?: number
}

export interface Stats {
  total_services: number
  total_tx: number
  total_rx: number
  total_conns: number
  total_clients: number
}

export interface Client {
  id: string
  services: string[]
  service_count: number
  active_conns: number
  services_detail: ClientService[]
}

export interface ClientService {
  name: string
  type: string
  remote_port: number
  conns: number
  healthy: boolean
}

export interface LogEntry {
  timestamp: number
  level: string
  message: string
}

export interface MetricPoint {
  ts: number
  total_tx: number
  total_rx: number
  total_conns: number
  active_clients: number
  active_services: number
  bandwidth_tx: number
  bandwidth_rx: number
}

export interface ServiceMetricPoint {
  ts: number
  conns: number
  tx: number
  rx: number
  bw_tx: number
  bw_rx: number
}

class AuthError extends Error {
  constructor() {
    super('Unauthorized')
    this.name = 'AuthError'
  }
}

async function request<T>(path: string, init?: RequestInit, token?: string): Promise<T> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(init?.headers as Record<string, string> | undefined),
  }
  if (token) {
    headers['Authorization'] = `Bearer ${token}`
  }
  const res = await fetch(path, { ...init, headers, credentials: token ? undefined : 'include' })
  if (res.status === 401) throw new AuthError()
  if (res.status === 204) return undefined as T
  if (!res.ok) {
    let msg = `Request failed (${res.status})`
    try {
      const body = await res.json()
      if (body?.error) msg = body.error
    } catch {}
    throw new Error(msg)
  }
  return (await res.json()) as T
}

export const api = {
  login(username: string, password: string) {
    return request<{ ok: boolean }>('/api/login', {
      method: 'POST',
      body: JSON.stringify({ user: username, password }),
    })
  },

  services(): Promise<{ services: Service[] }> {
    return request<{ services: Service[] }>('/api/services')
  },

  serviceDetail(name: string): Promise<{ service: Service }> {
    return request<{ service: Service }>(`/api/services/${encodeURIComponent(name)}`)
  },

  serviceMetrics(name: string, limit = 120): Promise<{ metrics: ServiceMetricPoint[] }> {
    return request<{ metrics: ServiceMetricPoint[] }>(`/api/services/${encodeURIComponent(name)}/metrics?limit=${limit}`)
  },

  stats(): Promise<Stats> {
    return request<Stats>('/api/stats')
  },

  metrics(limit = 120): Promise<{ metrics: MetricPoint[] }> {
    return request<{ metrics: MetricPoint[] }>(`/api/metrics?limit=${limit}`)
  },

  clients(): Promise<{ clients: Client[] }> {
    return request<{ clients: Client[] }>('/api/clients')
  },

  logs(): Promise<{ logs: LogEntry[] }> {
    return request<{ logs: LogEntry[] }>('/api/logs')
  },

  toggleService(name: string, enabled: boolean): Promise<{ ok: boolean }> {
    return request<{ ok: boolean }>(`/api/services/${encodeURIComponent(name)}/toggle`, {
      method: 'POST',
      body: JSON.stringify({ enabled }),
    })
  },

  kickClient(clientId: string): Promise<{ ok: boolean }> {
    return request<{ ok: boolean }>(`/api/clients/${encodeURIComponent(clientId)}/kick`, {
      method: 'POST',
    })
  },

  config(): Promise<Record<string, unknown>> {
    return request<Record<string, unknown>>('/api/config')
  },

  reloadConfig(): Promise<{ ok: boolean }> {
    return request<{ ok: boolean }>('/api/config/reload', { method: 'POST' })
  },

  connectWebSocket(onMessage: (ev: ServerEvent) => void): WebSocket {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const ws = new WebSocket(`${protocol}//${window.location.host}/api/ws`)
    ws.onmessage = (e) => {
      try {
        const ev = JSON.parse(e.data) as ServerEvent
        onMessage(ev)
      } catch {}
    }
    return ws
  },
}

export type ServerEvent =
  | { type: 'ServiceUpdate'; data: Service[] }
  | { type: 'Stats'; data: Stats }
  | { type: 'Log'; data: LogEntry }

export function formatBytes(n: number): string {
  if (!n || n < 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1)
  const val = n / Math.pow(1024, i)
  return val.toFixed(i === 0 ? 0 : 2) + ' ' + units[i]
}

export function formatBits(n: number): string {
  if (!n || n < 0) return '0 bps'
  const units = ['bps', 'Kbps', 'Mbps', 'Gbps', 'Tbps']
  const bits = n * 8
  if (bits <= 0) return '0 bps'
  const i = Math.min(Math.floor(Math.log(bits) / Math.log(1024)), units.length - 1)
  const val = bits / Math.pow(1024, i)
  return val.toFixed(i === 0 ? 0 : 1) + ' ' + units[i]
}

export function formatRate(bytesPerSec: number): string {
  return formatBits(bytesPerSec) + '/s'
}

export function formatUptime(seconds: number): string {
  if (!seconds || seconds < 0) return '0s'
  const d = Math.floor(seconds / 86400)
  const h = Math.floor((seconds % 86400) / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  if (d > 0) return `${d}d ${h}h`
  if (h > 0) return `${h}h ${m}m`
  if (m > 0) return `${m}m`
  return `${Math.floor(seconds)}s`
}

export function formatTime(ts: number): string {
  const d = new Date(ts * 1000)
  return d.toLocaleTimeString()
}
