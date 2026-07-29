import { useState, useEffect } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { Login } from './pages/Login'
import { Dashboard } from './pages/Dashboard'
import { api } from './api'

export default function App() {
  const [authed, setAuthed] = useState<boolean>(false)
  const [checking, setChecking] = useState<boolean>(true)

  useEffect(() => {
    let alive = true
    async function check() {
      try {
        await api.stats()
        if (alive) setAuthed(true)
      } catch {
        if (alive) setAuthed(false)
      } finally {
        if (alive) setChecking(false)
      }
    }
    check()
    return () => { alive = false }
  }, [])

  function handleAuth() {
    setAuthed(true)
  }

  function handleLogout() {
    setAuthed(false)
  }

  if (checking) {
    return (
      <div style={{
        position: 'fixed',
        inset: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'var(--bg)',
      }}>
        <motion.div
          initial={{ opacity: 0, scale: 0.8 }}
          animate={{ opacity: 1, scale: 1 }}
          style={{
            width: 40,
            height: 40,
            borderRadius: 10,
            background: 'linear-gradient(135deg, var(--accent), #5E5CE6)',
          }}
        />
      </div>
    )
  }

  return (
    <AnimatePresence mode="wait">
      {authed ? (
        <motion.div
          key="dashboard"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.25 }}
          style={{ height: '100%' }}
        >
          <Dashboard onLogout={handleLogout} />
        </motion.div>
      ) : (
        <motion.div
          key="login"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.25 }}
          style={{ height: '100%' }}
        >
          <Login onAuth={handleAuth} />
        </motion.div>
      )}
    </AnimatePresence>
  )
}
