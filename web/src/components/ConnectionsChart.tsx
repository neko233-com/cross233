import { useMemo } from 'react'
import { EChart } from './EChart'
import type { MetricPoint } from '../api'

interface ConnectionsChartProps {
  history: MetricPoint[]
  height?: number
}

export function ConnectionsChart({ history, height = 260 }: ConnectionsChartProps) {
  const option = useMemo(() => {
    const times = history.map((p) =>
      new Date(p.ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
    )
    const connsData = history.map((p) => p.total_conns)
    const clientsData = history.map((p) => p.active_clients)
    const servicesData = history.map((p) => p.active_services)

    return {
      backgroundColor: 'transparent',
      grid: { top: 30, right: 20, bottom: 30, left: 40 },
      tooltip: {
        trigger: 'axis',
        backgroundColor: 'rgba(30,30,32,0.92)',
        borderColor: 'rgba(255,255,255,0.08)',
        textStyle: { color: '#f5f5f7', fontSize: 12 },
      },
      legend: {
        data: ['连接数', '客户端', '服务'],
        top: 0,
        right: 10,
        textStyle: { color: '#aeaeb2', fontSize: 11 },
        itemWidth: 10,
        itemHeight: 10,
        icon: 'circle',
      },
      xAxis: {
        type: 'category',
        data: times,
        axisLine: { lineStyle: { color: 'rgba(255,255,255,0.08)' } },
        axisLabel: { color: '#8e8e93', fontSize: 10, hideOverlap: true },
        axisTick: { show: false },
        boundaryGap: false,
      },
      yAxis: {
        type: 'value',
        minInterval: 1,
        axisLine: { show: false },
        axisTick: { show: false },
        axisLabel: { color: '#8e8e93', fontSize: 10 },
        splitLine: { lineStyle: { color: 'rgba(255,255,255,0.05)', type: 'dashed' } },
      },
      series: [
        {
          name: '连接数',
          type: 'line',
          data: connsData,
          smooth: true,
          symbol: 'none',
          lineStyle: { color: '#BF5AF2', width: 2 },
          areaStyle: {
            color: {
              type: 'linear',
              x: 0, y: 0, x2: 0, y2: 1,
              colorStops: [
                { offset: 0, color: 'rgba(191,90,242,0.2)' },
                { offset: 1, color: 'rgba(191,90,242,0)' },
              ],
            },
          },
        },
        {
          name: '客户端',
          type: 'line',
          data: clientsData,
          smooth: true,
          symbol: 'none',
          step: 'end',
          lineStyle: { color: '#FF9F0A', width: 2 },
        },
        {
          name: '服务',
          type: 'line',
          data: servicesData,
          smooth: true,
          symbol: 'none',
          step: 'end',
          lineStyle: { color: '#64D2FF', width: 2 },
        },
      ],
      animation: false,
    } as echarts.EChartsOption
  }, [history])

  return <EChart option={option} height={height} />
}
