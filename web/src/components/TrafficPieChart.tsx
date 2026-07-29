import { useMemo } from 'react'
import { EChart } from './EChart'
import { formatBytes } from '../api'
import type { Service } from '../api'

interface TrafficPieChartProps {
  services: Service[]
  height?: number
}

export function TrafficPieChart({ services, height = 260 }: TrafficPieChartProps) {
  const option = useMemo(() => {
    const data = services
      .filter((s) => s.traffic_tx + s.traffic_rx > 0)
      .sort((a, b) => (b.traffic_tx + b.traffic_rx) - (a.traffic_tx + a.traffic_rx))
      .slice(0, 8)
      .map((s) => ({
        name: s.name,
        value: s.traffic_tx + s.traffic_rx,
      }))

    const colors = ['#0A84FF', '#30D158', '#BF5AF2', '#FF9F0A', '#64D2FF', '#FF375F', '#FFD60A', '#30D158']

    return {
      backgroundColor: 'transparent',
      tooltip: {
        trigger: 'item',
        backgroundColor: 'rgba(30,30,32,0.92)',
        borderColor: 'rgba(255,255,255,0.08)',
        textStyle: { color: '#f5f5f7', fontSize: 12 },
        formatter: (p: any) => `${p.name}<br/><b>${formatBytes(p.value)}</b> (${p.percent}%)`,
      },
      legend: {
        orient: 'vertical',
        right: 10,
        top: 'center',
        textStyle: { color: '#aeaeb2', fontSize: 11 },
        itemWidth: 10,
        itemHeight: 10,
        icon: 'circle',
      },
      color: colors,
      series: [
        {
          name: '流量分布',
          type: 'pie',
          radius: ['45%', '70%'],
          center: ['35%', '50%'],
          avoidLabelOverlap: true,
          itemStyle: {
            borderRadius: 6,
            borderColor: 'rgba(28,28,30,0.8)',
            borderWidth: 2,
          },
          label: { show: false },
          emphasis: {
            label: { show: true, fontSize: 12, fontWeight: 'bold', color: '#f5f5f7' },
            scaleSize: 8,
          },
          labelLine: { show: false },
          data,
        },
      ],
    } as echarts.EChartsOption
  }, [services])

  return <EChart option={option} height={height} />
}
