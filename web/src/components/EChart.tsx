import { useEffect, useRef } from 'react'
import * as echarts from 'echarts'

interface EChartProps {
  option: echarts.EChartsOption
  height?: number | string
  theme?: 'dark' | 'light'
  className?: string
}

export function EChart({ option, height = 280, theme = 'dark', className }: EChartProps) {
  const ref = useRef<HTMLDivElement>(null)
  const chartRef = useRef<echarts.ECharts | null>(null)

  useEffect(() => {
    if (!ref.current) return
    const chart = echarts.init(ref.current, theme, { renderer: 'canvas' })
    chartRef.current = chart

    const handleResize = () => chart.resize()
    window.addEventListener('resize', handleResize)

    return () => {
      window.removeEventListener('resize', handleResize)
      chart.dispose()
      chartRef.current = null
    }
  }, [theme])

  useEffect(() => {
    if (chartRef.current) {
      chartRef.current.setOption(option, { notMerge: false, lazyUpdate: true })
    }
  }, [option])

  return (
    <div
      ref={ref}
      className={className}
      style={{
        width: '100%',
        height: typeof height === 'number' ? `${height}px` : height,
      }}
    />
  )
}
