import { useEffect, useRef } from 'react'
import { motion, useMotionValue, useSpring, AnimatePresence } from 'framer-motion'
import type { ReactNode } from 'react'

interface StatCardProps {
  label: string
  value: number
  icon?: ReactNode
  format?: (n: number) => string
  suffix?: string
}

function AnimatedNumber({ value, format }: { value: number; format?: (n: number) => string }) {
  const mv = useMotionValue(0)
  const spring = useSpring(mv, { stiffness: 100, damping: 20, mass: 0.8 })
  const ref = useRef<HTMLSpanElement>(null)

  useEffect(() => {
    mv.set(value)
  }, [mv, value])

  useEffect(() => {
    const unsubscribe = spring.on('change', (v) => {
      if (ref.current) {
        ref.current.textContent = format ? format(Math.round(v)) : Math.round(v).toLocaleString()
      }
    })
    return () => unsubscribe()
  }, [spring, format])

  return <span ref={ref}>{format ? format(0) : '0'}</span>
}

export function StatCard({ label, value, icon, format, suffix }: StatCardProps) {
  return (
    <motion.div
      className="stat-card card"
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: 'spring', stiffness: 300, damping: 25 }}
    >
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start' }}>
        <div>
          <div className="stat-label">{label}</div>
          <div className="stat-value">
            <AnimatedNumber value={value} format={format} />
            {suffix && <span className="stat-unit">{suffix}</span>}
          </div>
        </div>
        {icon && <div className="stat-icon">{icon}</div>}
      </div>
    </motion.div>
  )
}
