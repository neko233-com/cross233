import { useEffect, useState, useCallback } from 'react'
import { api, formatBytes, formatRate, formatUptime, type Service, type Client, type LogEntry, type MetricPoint, type ServerEvent } from '../api'
import { StatCard } from '../components/StatCard'
import { BandwidthChart } from '../components/BandwidthChart'
import { ConnectionsChart } from '../components/ConnectionsChart'
import { TrafficPieChart } from '../components/TrafficPieChart'
import { Toggle } from '../components/Toggle'
import { Layout } from '../components/Layout'

type Tab = 'dashboard' | 'services' | 'clients' | 'logs'

export function Dashboard({ onLogout }: { onLogout: () => void }) {
  const [tab, setTab] = useState<Tab>('dashboard')
  const [services, setServices] = useState<Service[]>([])
  const [clients, setClients] = useState<Client[]>([])
  const [logs, setLogs] = useState<LogEntry[]>([])
  const [metrics, setMetrics] = useState<MetricPoint[]>([])
  const [stats, setStats] = useState({ total_services: 0, total_tx: 0, total_rx: 0, total_conns: 0, total_clients: 0 })
  const [serverStartTime] = useState(() => Date.now())
  const [searchService, setSearchService] = useState('')

  const refreshAll = useCallback(async () => {
    try {
      const [svcRes, cliRes, logRes, metRes, stRes] = await Promise.all([
        api.services(),
        api.clients(),
        api.logs(),
        api.metrics(120),
        api.stats(),
      ])
      setServices(svcRes.services)
      setClients(cliRes.clients)
      setLogs(logRes.logs)
      setMetrics(metRes.metrics)
      setStats(stRes)
    } catch (e) {
      console.warn('refresh failed', e)
    }
  }, [])

  useEffect(() => {
    refreshAll()
    const interval = setInterval(refreshAll, 5000)
    return () => clearInterval(interval)
  }, [refreshAll])

  useEffect(() => {
    const handleEvent = (ev: ServerEvent) => {
      if (ev.type === 'ServiceUpdate') {
        setServices(ev.data)
      } else if (ev.type === 'Stats') {
        setStats(ev.data)
      } else if (ev.type === 'Log') {
        setLogs((prev) => [...prev.slice(-199), ev.data])
      }
    }
    const ws = api.connectWebSocket(handleEvent)
    return () => {
      ws.close()
    }
  }, [])

  const currentBw = metrics.length > 0 ? metrics[metrics.length - 1] : { bandwidth_tx: 0, bandwidth_rx: 0 }
  const uptime = Math.floor((Date.now() - serverStartTime) / 1000)

  async function handleToggle(name: string, enabled: boolean) {
    await api.toggleService(name, enabled)
    setServices((prev) => prev.map((s) => (s.name === name ? { ...s, enabled } : s)))
  }

  async function handleKick(clientId: string) {
    if (!confirm(`确定要断开客户端 ${clientId.slice(0, 8)}... 吗？`)) return
    await api.kickClient(clientId)
    setClients((prev) => prev.filter((c) => c.id !== clientId))
    setServices((prev) => prev.filter((s) => s.client_id !== clientId))
  }

  const filteredServices = services.filter((s) =>
    !searchService || s.name.toLowerCase().includes(searchService.toLowerCase())
  )

  return (
    <Layout tab={tab} setTab={setTab} onLogout={onLogout}>
      {tab === 'dashboard' && (
        <>
          <div className="page-header">
            <h1 className="page-title">数据大盘</h1>
            <p className="page-subtitle">Cross233 服务运行总览 · 实时监控</p>
          </div>

          <div className="stats-grid">
            <StatCard
              label="活跃服务"
              value={stats.total_services}
              format={(n) => n.toLocaleString()}
              icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><rect x="2" y="2" width="20" height="8" rx="2"/><rect x="2" y="14" width="20" height="8" rx="2"/><line x1="6" y1="6" x2="6.01" y2="6"/><line x1="6" y1="18" x2="6.01" y2="18"/></svg>}
            />
            <StatCard
              label="在线客户端"
              value={stats.total_clients}
              format={(n) => n.toLocaleString()}
              icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/></svg>}
            />
            <StatCard
              label="当前连接"
              value={stats.total_conns}
              format={(n) => n.toLocaleString()}
              icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>}
            />
            <StatCard
              label="上行带宽"
              value={currentBw.bandwidth_tx}
              format={(n) => formatRate(n)}
              icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><polyline points="23 6 13.5 15.5 8.5 10.5 1 18"/><polyline points="17 6 23 6 23 12"/></svg>}
            />
            <StatCard
              label="下行带宽"
              value={currentBw.bandwidth_rx}
              format={(n) => formatRate(n)}
              icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><polyline points="23 18 13.5 8.5 8.5 13.5 1 6"/><polyline points="17 18 23 18 23 12"/></svg>}
            />
            <StatCard
              label="总发送流量"
              value={stats.total_tx}
              format={formatBytes}
              icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="22" y1="2" x2="11" y2="13"/><polygon points="22 2 15 2 22 9 22 2"/><path d="M20 14v6a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h6"/></svg>}
            />
            <StatCard
              label="总接收流量"
              value={stats.total_rx}
              format={formatBytes}
              icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>}
            />
            <StatCard
              label="运行时间"
              value={uptime}
              format={formatUptime}
              icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>}
            />
          </div>

          <div className="section-grid">
            <div className="card section-card">
              <div className="section-header">
                <div className="section-title">实时带宽</div>
                <span className="live-indicator">● LIVE</span>
              </div>
              <BandwidthChart history={metrics} height={260} />
            </div>
            <div className="card section-card">
              <div className="section-header">
                <div className="section-title">连接数趋势</div>
                <span className="live-indicator">● LIVE</span>
              </div>
              <ConnectionsChart history={metrics} height={260} />
            </div>
          </div>

          <div className="section-grid">
            <div className="card section-card">
              <div className="section-header">
                <div className="section-title">流量分布 (Top 8)</div>
              </div>
              <TrafficPieChart services={services} height={280} />
            </div>
            <div className="card section-card">
              <div className="section-header">
                <div className="section-title">最近日志</div>
              </div>
              <div className="log-viewer" style={{ height: 280 }}>
                {logs.slice(-50).map((l, i) => (
                  <div className="log-line" key={i}>
                    <span className="log-time mono">{new Date(l.timestamp * 1000).toLocaleTimeString()}</span>
                    <span className={`log-level ${l.level.toLowerCase()}`}>{l.level}</span>
                    <span className="log-message">{l.message}</span>
                  </div>
                ))}
                {logs.length === 0 && <div style={{ color: 'var(--text-tertiary)', textAlign: 'center', padding: 40 }}>暂无日志</div>}
              </div>
            </div>
          </div>
        </>
      )}

      {tab === 'services' && (
        <>
          <div className="page-header">
            <h1 className="page-title">服务管理</h1>
            <p className="page-subtitle">共 {services.length} 个隧道服务</p>
          </div>

          <div className="card section-card" style={{ marginBottom: 16 }}>
            <input
              className="text-input"
              placeholder="🔍 搜索服务名称..."
              value={searchService}
              onChange={(e) => setSearchService(e.target.value)}
              style={{ maxWidth: 320, background: 'rgba(255,255,255,0.04)' }}
            />
          </div>

          <div className="card section-card">
            <div className="data-table">
              <div className="data-table-head" style={{ gridTemplateColumns: '2fr 80px 1.5fr 1fr 1fr 100px 80px 120px' }}>
                <div className="data-table-head-cell">名称</div>
                <div className="data-table-head-cell">类型</div>
                <div className="data-table-head-cell">本地地址</div>
                <div className="data-table-head-cell">端口/域名</div>
                <div className="data-table-head-cell">流量</div>
                <div className="data-table-head-cell">连接</div>
                <div className="data-table-head-cell">状态</div>
                <div className="data-table-head-cell" style={{ textAlign: 'right' }}>操作</div>
              </div>
              {filteredServices.map((s) => (
                <div className="data-table-row" key={s.name} style={{ gridTemplateColumns: '2fr 80px 1.5fr 1fr 1fr 100px 80px 120px' }}>
                  <div className="data-table-cell">
                    <span className="status-dot online" style={{ background: s.healthy ? 'var(--success)' : 'var(--danger)' }} />
                    <span className="mono" title={s.name}>{s.name}</span>
                  </div>
                  <div className="data-table-cell">
                    <span className={`tag tag-${s.ty}`}>{s.ty}</span>
                  </div>
                  <div className="data-table-cell mono" style={{ color: 'var(--text-secondary)' }}>{s.local_addr}</div>
                  <div className="data-table-cell mono">
                    {s.host ? s.host : s.subdomain ? `${s.subdomain}.*` : s.remote_port > 0 ? `:${s.remote_port}` : '-'}
                  </div>
                  <div className="data-table-cell" style={{ fontSize: 12 }}>
                    <span style={{ color: '#0A84FF' }}>↑{formatBytes(s.traffic_tx)}</span>
                    <span style={{ color: 'var(--text-tertiary)', margin: '0 4px' }}>/</span>
                    <span style={{ color: '#30D158' }}>↓{formatBytes(s.traffic_rx)}</span>
                  </div>
                  <div className="data-table-cell">{s.current_conns}</div>
                  <div className="data-table-cell">
                    <span style={{ color: s.enabled ? 'var(--success)' : 'var(--text-tertiary)', fontSize: 12 }}>
                      {s.enabled ? (s.healthy ? '健康' : '异常') : '已禁用'}
                    </span>
                  </div>
                  <div className="data-table-cell table-actions">
                    <Toggle on={s.enabled} onChange={(v) => handleToggle(s.name, v)} />
                  </div>
                </div>
              ))}
              {filteredServices.length === 0 && (
                <div className="data-table-empty">没有匹配的服务</div>
              )}
            </div>
          </div>
        </>
      )}

      {tab === 'clients' && (
        <>
          <div className="page-header">
            <h1 className="page-title">客户端</h1>
            <p className="page-subtitle">共 {clients.length} 个客户端在线</p>
          </div>

          <div className="clients-list">
            {clients.map((c) => (
              <div className="card section-card" key={c.id} style={{ marginBottom: 12 }}>
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 12 }}>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
                    <span className="status-dot online" />
                    <span className="mono" style={{ fontSize: 14, fontWeight: 600 }}>
                      {c.id.slice(0, 12)}...
                    </span>
                    <span className="tag tag-tcp" style={{ fontSize: 10 }}>
                      {c.service_count} 服务 · {c.active_conns} 连接
                    </span>
                  </div>
                  <button className="btn-danger" onClick={() => handleKick(c.id)}>断开</button>
                </div>
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
                  {c.services_detail.map((svc) => (
                    <div key={svc.name} className="tag" style={{
                      background: svc.healthy ? 'rgba(48,209,88,0.1)' : 'rgba(255,69,58,0.1)',
                      color: svc.healthy ? '#30D158' : '#FF453A',
                      textTransform: 'none',
                      letterSpacing: 0,
                      fontSize: 12,
                      fontWeight: 500,
                      padding: '4px 10px',
                      fontFamily: 'var(--font-mono)',
                    }}>
                      {svc.name} · :{svc.remote_port} · {svc.conns} conns
                    </div>
                  ))}
                </div>
              </div>
            ))}
            {clients.length === 0 && (
              <div className="card section-card" style={{ textAlign: 'center', padding: '60px 20px', color: 'var(--text-tertiary)' }}>
                暂无客户端连接
              </div>
            )}
          </div>
        </>
      )}

      {tab === 'logs' && (
        <>
          <div className="page-header">
            <h1 className="page-title">运行日志</h1>
            <p className="page-subtitle">最近 {logs.length} 条日志</p>
          </div>

          <div className="card section-card">
            <div className="log-viewer" style={{ height: 'calc(100vh - 240px)' }}>
              {logs.map((l, i) => (
                <div className="log-line" key={i}>
                  <span className="log-time mono">{new Date(l.timestamp * 1000).toLocaleString()}</span>
                  <span className={`log-level ${l.level.toLowerCase()}`}>{l.level}</span>
                  <span className="log-message">{l.message}</span>
                </div>
              ))}
              {logs.length === 0 && <div style={{ color: 'var(--text-tertiary)', textAlign: 'center', padding: 60 }}>暂无日志</div>}
            </div>
          </div>
        </>
      )}
    </Layout>
  )
}
