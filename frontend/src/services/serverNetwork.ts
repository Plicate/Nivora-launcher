import type { ServerNetworkReport } from '../types'

export function publicIpv4(value: string): boolean {
  const parts = value.trim().split('.')
  if (parts.length !== 4 || parts.some((part) => !/^\d{1,3}$/.test(part) || Number(part) > 255 || String(Number(part)) !== part)) return false
  const [a, b, c] = parts.map(Number)
  return a > 0 && a < 224 && a !== 10 && a !== 127
    && !(a === 100 && b >= 64 && b <= 127)
    && !(a === 169 && b === 254) && !(a === 172 && b >= 16 && b <= 31)
    && !(a === 192 && (b === 168 || b === 0 && (c === 0 || c === 2)))
    && !(a === 198 && (b === 18 || b === 19 || b === 51 && c === 100))
    && !(a === 203 && b === 0 && c === 113)
}

export function serverAddress(ip: string, port: number): string {
  return publicIpv4(ip) && Number.isInteger(port) && port > 0 && port <= 65535 ? `${ip.trim()}:${port}` : ''
}

export function canMapServer(report: ServerNetworkReport | undefined, running: boolean): boolean {
  return Boolean(
    report?.upnp_available
      && report.bind_all
      && report.local_listening
      && report.running === running
      && running
      && report.nat_kind === 'public',
  )
}

export const natLabels: Record<ServerNetworkReport['nat_kind'], string> = {
  direct: 'Публичный IPv4 на компьютере',
  public: 'Белый IPv4 на роутере',
  double_nat: 'Другой внешний NAT или VPN',
  private: 'Серый WAN / второй роутер',
  cgnat: 'CGNAT провайдера',
  unknown: 'Тип подключения не определён',
}
