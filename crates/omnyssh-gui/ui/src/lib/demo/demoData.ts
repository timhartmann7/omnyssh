import type { HostDto, ConnectionStatusDto, MetricsDto } from '$lib/bindings';
import { hosts } from '$lib/stores/hosts';
import { statuses } from '$lib/stores/statuses';
import { metrics } from '$lib/stores/metrics';
import { services, type HostServices } from '$lib/stores/services';
import { sessions } from '$lib/stores/sessions';

export const DEMO_HOSTS: HostDto[] = [
  {
    name: 'prod-web-1',
    hostname: '10.0.1.12',
    port: 22,
    user: 'deploy',
    tags: ['prod', 'web'],
    source: 'manual',
    hasKey: true,
    passwordAuthDisabled: true,
    monitoring: 'ssh'
  },
  {
    name: 'prod-db-primary',
    hostname: '10.0.1.20',
    port: 22,
    user: 'postgres',
    tags: ['prod', 'db'],
    source: 'sshConfig',
    hasKey: true,
    passwordAuthDisabled: false,
    monitoring: 'ssh'
  },
  {
    name: 'staging-api',
    hostname: '192.168.1.105',
    port: 2222,
    user: 'ubuntu',
    tags: ['staging'],
    source: 'manual',
    hasKey: true,
    passwordAuthDisabled: false,
    monitoring: 'ssh'
  },
  {
    name: 'cache-cluster-1',
    hostname: '10.0.2.15',
    port: 22,
    user: 'root',
    tags: ['redis', 'cluster'],
    source: 'manual',
    hasKey: true,
    passwordAuthDisabled: true,
    monitoring: 'ssh'
  },
  {
    name: 'backup-node-eu',
    hostname: '198.51.100.42',
    port: 22,
    user: 'backup',
    tags: ['backup', 'eu'],
    source: 'manual',
    hasKey: true,
    passwordAuthDisabled: false,
    monitoring: 'ssh'
  },
  {
    name: 'monitoring-edge',
    hostname: '10.0.99.1',
    port: 22,
    user: 'admin',
    tags: ['monitoring'],
    source: 'manual',
    hasKey: true,
    passwordAuthDisabled: false,
    monitoring: 'tcpPort',
    monitorPort: 9100
  }
];

export function seedDemoData(): void {
  hosts.set(DEMO_HOSTS);

  const statusMap = new Map<string, ConnectionStatusDto>();
  DEMO_HOSTS.forEach((h) => statusMap.set(h.name, { kind: 'connected' }));
  statuses.set(statusMap);

  const metricsMap = new Map<string, MetricsDto>();
  metricsMap.set('prod-web-1', {
    cpuPercent: 12,
    ramPercent: 48,
    diskPercent: 62,
    uptime: '42d 18h',
    loadAvg: '0.35, 0.42, 0.38',
    osInfo: 'Ubuntu 24.04.1 LTS',
    topProcesses: [
      { name: 'nginx', cpuPercent: 6.2, memPercent: 1.2 },
      { name: 'node', cpuPercent: 4.8, memPercent: 3.5 }
    ],
    ageSeconds: 2
  });

  metricsMap.set('prod-db-primary', {
    cpuPercent: 28,
    ramPercent: 76,
    diskPercent: 41,
    uptime: '118d 04h',
    loadAvg: '1.20, 0.95, 0.88',
    osInfo: 'Debian 12.7 (bookworm)',
    topProcesses: [
      { name: 'postgres', cpuPercent: 22.4, memPercent: 18.2 },
      { name: 'wal-g', cpuPercent: 4.1, memPercent: 0.8 }
    ],
    ageSeconds: 1
  });

  metricsMap.set('staging-api', {
    cpuPercent: 5,
    ramPercent: 32,
    diskPercent: 22,
    uptime: '14d 06h',
    loadAvg: '0.10, 0.15, 0.12',
    osInfo: 'Ubuntu 22.04.4 LTS',
    topProcesses: [
      { name: 'api-server', cpuPercent: 3.5, memPercent: 4.2 },
      { name: 'docker-proxy', cpuPercent: 1.2, memPercent: 0.5 }
    ],
    ageSeconds: 3
  });

  metricsMap.set('cache-cluster-1', {
    cpuPercent: 3,
    ramPercent: 18,
    diskPercent: 15,
    uptime: '89d 21h',
    loadAvg: '0.05, 0.08, 0.06',
    osInfo: 'Alpine Linux 3.19',
    topProcesses: [{ name: 'redis-server', cpuPercent: 2.8, memPercent: 2.1 }],
    ageSeconds: 2
  });

  metricsMap.set('backup-node-eu', {
    cpuPercent: 88,
    ramPercent: 24,
    diskPercent: 82,
    uptime: '310d 12h',
    loadAvg: '2.80, 2.40, 2.10',
    osInfo: 'Debian 12.5',
    topProcesses: [
      { name: 'borg', cpuPercent: 82.0, memPercent: 4.1 },
      { name: 'restic', cpuPercent: 5.1, memPercent: 1.2 }
    ],
    ageSeconds: 1
  });

  metrics.set(metricsMap);

  const servicesMap = new Map<string, HostServices>();
  servicesMap.set('prod-web-1', {
    kind: 'detected',
    services: [
      {
        kind: 'docker',
        metrics: [
          { name: 'containers_total', value: 4 },
          { name: 'containers_running', value: 4 }
        ]
      },
      { kind: 'nginx', metrics: [] }
    ]
  });

  servicesMap.set('prod-db-primary', {
    kind: 'detected',
    services: [
      { kind: 'postgresql', metrics: [] },
      { kind: 'redis', metrics: [] }
    ]
  });

  servicesMap.set('staging-api', {
    kind: 'detected',
    services: [
      {
        kind: 'docker',
        metrics: [
          { name: 'containers_total', value: 2 },
          { name: 'containers_running', value: 2 }
        ]
      }
    ]
  });

  servicesMap.set('cache-cluster-1', {
    kind: 'detected',
    services: [{ kind: 'redis', metrics: [] }]
  });

  services.set(servicesMap);

  // Seed sample sessions in sidebar
  sessions.set([
    { id: 1, kind: 'terminal', hostName: 'prod-web-1', status: 'connected' },
    { id: 2, kind: 'sftp', hostName: 'prod-db-primary', status: 'connected' }
  ]);
}
