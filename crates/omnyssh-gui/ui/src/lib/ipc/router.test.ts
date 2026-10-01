import { beforeEach, describe, expect, it } from 'vitest';
import { get } from 'svelte/store';
import type { HostDto } from '$lib/bindings';
import { hosts } from '$lib/stores/hosts';
import { statuses } from '$lib/stores/statuses';
import { metrics } from '$lib/stores/metrics';
import { services } from '$lib/stores/services';
import { tunnels } from '$lib/stores/tunnels';
import { snippetRun, beginRun, clearRun } from '$lib/stores/snippets';
import { sessions } from '$lib/stores/sessions';
import { lastError } from '$lib/stores/notifications';
import { keySetup, dismissKeySetup, beginKeySetup } from '$lib/stores/keySetup';
import { passphrasePrompt, passphraseQueue } from '$lib/stores/passphrase';
import {
  applyError,
  applyHostStatusChanged,
  applyHostsLoaded,
  applyKeyPassphraseRequired,
  applyKeySetupComplete,
  applyKeySetupFailed,
  applyKeySetupProgress,
  applyKeySetupRollback,
  applyMetricsUpdated,
  applyServicesDetected,
  applyServicesFailed,
  applySnippetResult,
  applyTerminalExited,
  applyTunnelStatusChanged,
  terminalDidExit
} from './router';

describe('ipc event router', () => {
  beforeEach(() => {
    hosts.set([]);
    statuses.set(new Map());
    metrics.set(new Map());
    services.set(new Map());
    tunnels.set(new Map());
    lastError.set(null);
    passphraseQueue.set([]);
  });

  it('routes a hosts-loaded payload into the hosts store', () => {
    const payload: HostDto[] = [
      {
        name: 'web-1',
        hostname: '10.0.0.1',
        user: 'root',
        port: 22,
        tags: [],
        source: 'manual',
        hasKey: false,
        monitoring: 'ssh',
        localForwards: [],
        tunnelAutostart: false,
        forwardAgent: false
      }
    ];

    applyHostsLoaded(payload);

    expect(get(hosts)).toEqual(payload);
  });

  it('routes a host-status-changed payload into the statuses store', () => {
    applyHostStatusChanged({ hostName: 'web-1', status: { kind: 'connected' } });
    applyHostStatusChanged({ hostName: 'web-2', status: { kind: 'failed', message: 'down' } });

    const map = get(statuses);
    expect(map.get('web-1')).toEqual({ kind: 'connected' });
    expect(map.get('web-2')).toEqual({ kind: 'failed', message: 'down' });
  });

  it('replaces a host status on the next change', () => {
    applyHostStatusChanged({ hostName: 'web-1', status: { kind: 'connecting' } });
    applyHostStatusChanged({ hostName: 'web-1', status: { kind: 'connected' } });

    expect(get(statuses).get('web-1')).toEqual({ kind: 'connected' });
    expect(get(statuses).size).toBe(1);
  });

  it('routes a metrics-updated payload into the metrics store', () => {
    applyMetricsUpdated({
      hostName: 'web-1',
      metrics: { cpuPercent: 12.5, topProcesses: [], ageSeconds: 0 }
    });

    expect(get(metrics).get('web-1')?.cpuPercent).toBe(12.5);
  });

  it('merges a partial metrics update, preserving prior fields', () => {
    // The poller sends a full snapshot...
    applyMetricsUpdated({
      hostName: 'web-1',
      metrics: {
        cpuPercent: 90,
        ramPercent: 10,
        diskPercent: 5,
        topProcesses: [{ name: 'pg', cpuPercent: 50, memPercent: 20 }],
        ageSeconds: 0
      }
    });
    // ...then discovery sends an os-info-only partial (cpu/ram/disk absent).
    applyMetricsUpdated({
      hostName: 'web-1',
      metrics: { osInfo: 'Ubuntu 22.04', topProcesses: [], ageSeconds: 0 }
    });

    const m = get(metrics).get('web-1');
    expect(m?.cpuPercent).toBe(90); // not wiped — this is what keeps the alert count stable
    expect(m?.ramPercent).toBe(10);
    expect(m?.osInfo).toBe('Ubuntu 22.04');
    expect(m?.topProcesses).toHaveLength(1);
  });

  it('prunes status/metrics/services for hosts dropped on reload', () => {
    applyHostStatusChanged({ hostName: 'web-1', status: { kind: 'connected' } });
    applyHostStatusChanged({ hostName: 'web-2', status: { kind: 'connected' } });
    applyMetricsUpdated({ hostName: 'web-2', metrics: { cpuPercent: 90, topProcesses: [], ageSeconds: 0 } });
    applyServicesDetected({ hostName: 'web-2', services: [{ kind: 'docker', metrics: [] }] });

    applyHostsLoaded([
      { name: 'web-1', hostname: '10.0.0.1', user: 'root', port: 22, tags: [], source: 'manual', hasKey: false, monitoring: 'ssh', localForwards: [], tunnelAutostart: false, forwardAgent: false }
    ]);

    expect(get(statuses).has('web-2')).toBe(false);
    expect(get(metrics).has('web-2')).toBe(false);
    expect(get(services).has('web-2')).toBe(false);
    expect(get(statuses).get('web-1')).toEqual({ kind: 'connected' });
  });

  it('routes a services-detected payload into the services store', () => {
    applyServicesDetected({
      hostName: 'web-1',
      services: [{ kind: 'docker', metrics: [{ name: 'containers_running', value: 4 }] }]
    });

    expect(get(services).get('web-1')).toEqual({
      kind: 'detected',
      services: [{ kind: 'docker', metrics: [{ name: 'containers_running', value: 4 }] }]
    });
  });

  it('routes a services-failed payload, replacing a prior detection', () => {
    applyServicesDetected({ hostName: 'web-1', services: [{ kind: 'nginx', metrics: [] }] });
    applyServicesFailed({ hostName: 'web-1', message: 'scan timed out' });

    expect(get(services).get('web-1')).toEqual({ kind: 'failed', message: 'scan timed out' });
    expect(get(services).size).toBe(1);
  });

  it('routes an error payload into the notifications store', () => {
    applyError('boom');

    expect(get(lastError)).toBe('boom');
  });

  it('routes a snippet-result into the active run, keyed by host', () => {
    beginRun('deploy', ['web-1', 'web-2']);
    applySnippetResult({ hostName: 'web-2', snippetName: 'deploy', ok: true, output: 'done' });

    const run = get(snippetRun);
    expect(run?.entries[0].pending).toBe(true); // web-1 untouched
    expect(run?.entries[1]).toEqual({ hostName: 'web-2', pending: false, ok: true, output: 'done' });
    clearRun();
  });

  it('terminal-exited without output closes the tab matched by backend id', () => {
    const tab = sessions.spawn('terminal', 'web-1');
    sessions.setTermId(tab.id, 501);

    applyTerminalExited(501, false);

    expect(get(sessions).some((s) => s.id === tab.id)).toBe(false);
  });

  it('terminal-exited after output keeps the tab, marked closed', () => {
    const tab = sessions.spawn('terminal', 'web-1');
    const other = sessions.spawn('terminal', 'db-1');
    sessions.setTermId(tab.id, 502);
    sessions.setTermId(other.id, 503);
    sessions.setStatus(tab.id, 'connected');
    sessions.setStatus(other.id, 'connected');

    applyTerminalExited(502, true);

    const list = get(sessions);
    expect(list.find((s) => s.id === tab.id)?.status).toBe('closed');
    expect(list.find((s) => s.id === other.id)?.status).toBe('connected');
    sessions.close(tab.id);
    sessions.close(other.id);
  });

  it('a terminal-exited that races ahead of terminalOpen reconciles on setTermId', () => {
    // The exit fires before any tab recorded termId 777, so it is parked with its
    // hadOutput...
    applyTerminalExited(777, false);
    applyTerminalExited(778, true);
    // ...then the tab records its id and learns how it exited (consumed once).
    expect(terminalDidExit(777)).toBe(false);
    expect(terminalDidExit(777)).toBeUndefined();
    expect(terminalDidExit(778)).toBe(true);
    expect(terminalDidExit(778)).toBeUndefined();
    // A session that is still running has nothing parked.
    expect(terminalDidExit(779)).toBeUndefined();
  });

  it('routes key-setup progress into the active run, then a terminal outcome', () => {
    beginKeySetup('web-1');
    applyKeySetupProgress({
      hostName: 'web-1',
      step: { index: 3, total: 6, description: 'Verifying key authentication' }
    });
    expect(get(keySetup)).toEqual({
      hostName: 'web-1',
      phase: { kind: 'running', step: { index: 3, total: 6, description: 'Verifying key authentication' } }
    });

    applyKeySetupComplete({ hostName: 'web-1', keyPath: '/k/id_ed25519' });
    expect(get(keySetup)).toEqual({
      hostName: 'web-1',
      phase: { kind: 'complete', keyPath: '/k/id_ed25519' }
    });
    dismissKeySetup();
  });

  it('a key-setup failure/rollback shows for its host even with no open run', () => {
    dismissKeySetup(); // nothing open
    applyKeySetupFailed({ hostName: 'db-1', error: 'Connection failed' });
    expect(get(keySetup)).toEqual({ hostName: 'db-1', phase: { kind: 'failed', error: 'Connection failed' } });

    applyKeySetupRollback({ hostName: 'db-1', result: 'Restored.' });
    expect(get(keySetup)).toEqual({ hostName: 'db-1', phase: { kind: 'rolledBack', result: 'Restored.' } });
    dismissKeySetup();
  });

  it('queues key-passphrase-required prompts, one per key', () => {
    applyKeyPassphraseRequired({ hostName: 'web-1', keyPath: '/home/me/.ssh/id_ed25519' });
    applyKeyPassphraseRequired({ hostName: 'db-1', keyPath: '/home/me/.ssh/other' });
    applyKeyPassphraseRequired({ hostName: 'web-2', keyPath: '/home/me/.ssh/id_ed25519' });

    expect(get(passphrasePrompt)).toEqual({ hostName: 'web-1', keyPath: '/home/me/.ssh/id_ed25519' });
    expect(get(passphraseQueue).map((p) => p.keyPath)).toEqual([
      '/home/me/.ssh/id_ed25519',
      '/home/me/.ssh/other'
    ]);
  });

  it('keeps each host\'s latest tunnel status and forgets a stopped one', () => {
    applyTunnelStatusChanged({ hostName: 'nas', status: { kind: 'connecting' } });
    applyTunnelStatusChanged({ hostName: 'nas', status: { kind: 'up' } });
    applyTunnelStatusChanged({ hostName: 'db', status: { kind: 'retrying', message: 'connection lost' } });
    expect(get(tunnels).get('nas')).toEqual({ kind: 'up' });
    expect(get(tunnels).get('db')).toEqual({ kind: 'retrying', message: 'connection lost' });

    applyTunnelStatusChanged({ hostName: 'nas', status: { kind: 'stopped' } });
    expect(get(tunnels).has('nas')).toBe(false);
  });

  it('drops the tunnel status of a host that is gone', () => {
    applyTunnelStatusChanged({ hostName: 'old', status: { kind: 'failed', message: 'x' } });
    applyHostsLoaded([]);
    expect(get(tunnels).has('old')).toBe(false);
  });
});
