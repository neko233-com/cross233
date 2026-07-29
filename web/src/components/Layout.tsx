import { motion } from 'framer-motion'
import type { ReactNode } from 'react'
import {
  DashboardIcon,
  ServicesIcon,
  ClientsIcon,
  LogsIcon,
  LogoIcon,
} from './Icons'

export type TabKey = 'dashboard' | 'services' | 'clients' | 'logs'

interface LayoutProps {
  children: ReactNode
  tab: TabKey
  setTab: (t: TabKey) => void
  onLogout: () => void
}

const navItems: { key: TabKey; label: string; Icon: React.ComponentType<{ size?: number }> }[] = [
  { key: 'dashboard', label: '数据大盘', Icon: DashboardIcon },
  { key: 'services', label: '服务管理', Icon: ServicesIcon },
  { key: 'clients', label: '客户端', Icon: ClientsIcon },
  { key: 'logs', label: '运行日志', Icon: LogsIcon },
]

export function Layout({ children, tab, setTab, onLogout }: LayoutProps) {
  return (
    <div className="app-shell">
      <motion.aside
        className="sidebar"
        initial={{ x: -40, opacity: 0 }}
        animate={{ x: 0, opacity: 1 }}
        transition={{ type: 'spring', stiffness: 300, damping: 25 }}
      >
        <div className="sidebar-logo">
          <div className="sidebar-logo-mark">
            <LogoIcon size={18} />
          </div>
          <span className="sidebar-logo-text">cross233</span>
        </div>

        <nav className="sidebar-nav">
          {navItems.map(({ key, label, Icon }, i) => (
            <motion.button
              key={key}
              className={`sidebar-item ${tab === key ? 'active' : ''}`}
              onClick={() => setTab(key)}
              initial={{ opacity: 0, x: -12 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ type: 'spring', stiffness: 300, damping: 25, delay: 0.05 + i * 0.04 }}
              whileTap={{ scale: 0.97 }}
            >
              <span className="sidebar-item-icon">
                <Icon size={18} />
              </span>
              <span>{label}</span>
            </motion.button>
          ))}
        </nav>

        <div className="sidebar-footer">
          <motion.button
            className="sidebar-logout"
            onClick={onLogout}
            whileTap={{ scale: 0.97 }}
            initial={{ opacity: 0, x: -12 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ type: 'spring', stiffness: 300, damping: 25, delay: 0.3 }}
          >
            <span className="sidebar-item-icon">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>
            </span>
            <span>退出</span>
          </motion.button>
        </div>
      </motion.aside>

      <motion.main
        className="main-content"
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        transition={{ duration: 0.3, delay: 0.1 }}
      >
        {children}
      </motion.main>
    </div>
  )
}
