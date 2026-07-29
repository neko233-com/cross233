import { useState, type FormEvent } from 'react'
import { motion } from 'framer-motion'
import { api } from '../api'
import { LogoIcon } from '../components/Icons'

interface LoginProps {
  onAuth: () => void
}

export function Login({ onAuth }: LoginProps) {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)

  async function handleSubmit(e: FormEvent) {
    e.preventDefault()
    setLoading(true)
    setError('')
    try {
      const res = await api.login(username.trim(), password)
      if (res.ok) {
        onAuth()
      } else {
        setError('登录失败')
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : '连接失败')
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="login-screen">
      <motion.div
        className="login-card card"
        initial={{ opacity: 0, y: 24, scale: 0.96 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        transition={{ type: 'spring', stiffness: 250, damping: 28 }}
      >
        <motion.div
          className="login-logo"
          initial={{ scale: 0, rotate: -10 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: 'spring', stiffness: 300, damping: 20, delay: 0.15 }}
        >
          <LogoIcon size={28} />
        </motion.div>

        <motion.h1
          className="login-title"
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.25, duration: 0.3 }}
        >
          欢迎使用 cross233
        </motion.h1>

        <motion.p
          className="login-subtitle"
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.3, duration: 0.3 }}
        >
          反向代理隧道管理控制台
        </motion.p>

        <motion.form
          className="login-form"
          onSubmit={handleSubmit}
          initial={{ opacity: 0, y: 12 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.35, duration: 0.3 }}
        >
          <input
            type="text"
            className="text-input"
            placeholder="用户名"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoFocus
            autoComplete="username"
            disabled={loading}
          />
          <input
            type="password"
            className="text-input"
            placeholder="密码"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
            disabled={loading}
          />
          <div className="login-error">{error}</div>
          <motion.button
            type="submit"
            className="btn-primary"
            disabled={loading}
            whileHover={!loading ? { scale: 1.01 } : undefined}
            whileTap={{ scale: 0.98 }}
            style={{ width: '100%', padding: '12px', marginTop: '4px' }}
          >
            {loading ? '登录中...' : '登 录'}
          </motion.button>
        </motion.form>
      </motion.div>
    </div>
  )
}
