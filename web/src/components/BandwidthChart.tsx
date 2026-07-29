import { useMemo } from 'react'
import { EChart } from './EChart'
import { formatRate } from '../api'
import type { MetricPoint } from '../api'

interface BandwidthChartProps {
  history: MetricPoint[]
  height?: number
}

export function BandwidthChart({ history, height = 260 }: BandwidthChartProps) {
  const option = useMemo(() => {
    const times = history.map((p) =>
      new Date(p.ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
    )
    const txData = history.map((p) => +(p.bandwidth_tx / 1024).toFixed(2))
    const rxData = history.map((p) => +(p.bandwidth_rx / 1024).toFixed(2))

    return {
      backgroundColor: 'transparent',
      grid: { top: 20, right: 20, bottom: 30, left: 50 },
      tooltip: {
        trigger: 'axis',
        backgroundColor: 'rgba(30,30,32,0.92)',
        borderColor: 'rgba(255,255,255,0.08)',
        textStyle: { color: '#f5f5f7', fontSize: 12 },
        formatter: (params: any[]) => {
          if (!params?.length) return ''
          let html = `<div style="font-size:11px;opacity:0.6;margin-bottom:4px">${params[0].axisValue}</div>`
          for (const p of params) {
            html += `<div style="display:flex;align-items:center;gap:6px;margin:2px 0">
              <span style="display:inline-block;width:8px;height:8px;border-radius:50%;background:${p.color}"></span>
              <span>${p.seriesName}: ${formatRate(p.value * 1024)}</span>
            </div>`
          }
          return html
        },
      },
      legend: {
        data: ['上传 (TX)', '下载 (RX)'],
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
        name: 'KB/s',
        nameTextStyle: { color: '#8e8e93', fontSize: 10 },
        axisLine: { show: false },
        axisTick: { show: false },
        axisLabel: { color: '#8e8e93', fontSize: 10 },
        splitLine: { lineStyle: { color: 'rgba(255,255,255,0.05)', type: 'dashed' } },
      },
      series: [
        {
          name: '上传 (TX)',
          type: 'line',
          data: txData,
          smooth: true,
          symbol: 'none',
          lineStyle: { color: '#0A84FF', width: 2 },
          areaStyle: {
            color: {
              type: 'linear',
              x: 0, y: 0, x2: 0, y2: 1,
              colorStops: [
                { offset: 0, color: 'rgba(10,132,255,0.25)' },
                { offset: 1, color: 'rgba(10,132,255,0)' },
              ],
            },
          },
        },
        {
          name: '下载 (RX)',
          type: 'line',
          data: rxData,
          smooth: true,
          symbol: 'none',
          lineStyle: { color: '#30D158', width: 2 },
          areaStyle: {
            color: {
              type: 'linear',
              x: 0, y: 0, x2: 0, y2: 1,
              colorStops: [
                { offset: 0, color: 'rgba(48,209,88,0.2)' },
                { offset: 1, color: 'rgba(48,209,88,0)' },
              ],
            },
          },
        },
      ],
      animation: false,
    } as echarts.EChartsOption
  }, [history])

  return <EChart option={option} height={height} />
}
