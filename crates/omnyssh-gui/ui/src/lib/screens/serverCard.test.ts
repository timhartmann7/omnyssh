import { describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import type { ConnectionStatusDto, HostDto, MetricsDto } from '$lib/bindings';
import type { HostServices } from '$lib/stores/services';
import {
  deriveCard,
  deriveTunnel,
  filterHosts,
  groupByTag,
  moveCard,
  orderKeys,
  sortCards,
  forwardListen,
  forwardTarget,
  metricStatus,
  QUICK_ACTIONS
} from './serverCard';

function host(name = 'web-1'): HostDto {
  return { name, hostname: '10.0.0.1', user: 'root', port: 22, tags: [], source: 'manual', hasKey: false, monitoring: 'ssh', localForwards: [], tunnelAutostart: false, forwardAgent: false };
}

function tcpHost(name = 'fw-1'): HostDto {
  return { ...host(name), monitoring: 'tcpPort', monitorPort: 8443 };
}

function metrics(partial: Partial<MetricsDto>): MetricsDto {
  return { topProcesses: [], ageSeconds: 0, ...partial };
}

const CONNECTED: ConnectionStatusDto = { kind: 'connected' };

describe('metricStatus — mirrors metrics::threshold_level', () => {
  it('classifies against the 60 / 85 boundaries', () => {
    expect(metricStatus(0)).toBe('ok');
    expect(metricStatus(59.9)).toBe('ok');
    expect(metricStatus(60)).toBe('warn');
    expect(metricStatus(85)).toBe('warn');
    expect(metricStatus(85.1)).toBe('crit');
    expect(metricStatus(100)).toBe('crit');
  });
});

describe('deriveCard — health state', () => {
  it('a connected, healthy host is ok and not offline', () => {
    const card = deriveCard(host(), CONNECTED, metrics({ cpuPercent: 10, ramPercent: 20, diskPercent: 30 }), undefined);
    expect(card.overall).toBe('ok');
    expect(card.offline).toBe(false);
    expect(card.metricRows.map((r) => r.status)).toEqual(['ok', 'ok', 'ok']);
  });

  it('a connected host takes the worst metric severity as its overall state', () => {
    const warn = deriveCard(host(), CONNECTED, metrics({ cpuPercent: 10, ramPercent: 70, diskPercent: 10 }), undefined);
    expect(warn.overall).toBe('warn');
    const crit = deriveCard(host(), CONNECTED, metrics({ cpuPercent: 95, ramPercent: 70, diskPercent: 10 }), undefined);
    expect(crit.overall).toBe('crit');
  });

  it('a connected host with no metrics yet is ok and not offline', () => {
    const card = deriveCard(host(), CONNECTED, undefined, undefined);
    expect(card.overall).toBe('ok');
    expect(card.offline).toBe(false);
    expect(card.metricRows.map((r) => r.percent)).toEqual([null, null, null]);
    expect(card.metricRows.map((r) => r.status)).toEqual(['unknown', 'unknown', 'unknown']);
  });

  it('a failed host with no metrics is off and offline', () => {
    const card = deriveCard(host(), { kind: 'failed', message: 'refused' }, undefined, undefined);
    expect(card.overall).toBe('off');
    expect(card.offline).toBe(true);
  });

  it('says why a failed host is down, and nothing otherwise', () => {
    const failed: ConnectionStatusDto = { kind: 'failed', message: 'SSH connection failed: Connection refused' };
    expect(deriveCard(host(), failed, undefined, undefined).failure).toBe(failed.message);
    expect(deriveCard(host(), failed, metrics({ cpuPercent: 40 }), undefined).failure).toBe(failed.message);
    expect(deriveCard(tcpHost(), failed, undefined, undefined).failure).toBe(failed.message);
    expect(deriveCard(host(), CONNECTED, undefined, undefined).failure).toBeUndefined();
    expect(deriveCard(host(), { kind: 'connecting' }, undefined, undefined).failure).toBeUndefined();
  });

  it('a failed host keeps showing its last metrics rather than an offline state', () => {
    const card = deriveCard(host(), { kind: 'failed', message: 'refused' }, metrics({ cpuPercent: 40 }), undefined);
    expect(card.overall).toBe('off');
    expect(card.offline).toBe(false);
  });

  it('a failed host with only os-info renders (not offline), mirroring the TUI', () => {
    // Discovery emits an os-info-only sample; a down host still shows what it knows.
    const card = deriveCard(host(), { kind: 'failed', message: 'refused' }, metrics({ osInfo: 'Ubuntu 22.04' }), undefined);
    expect(card.offline).toBe(false);
    expect(card.osInfo).toBe('Ubuntu 22.04');
    expect(card.overall).toBe('off');
  });

  it('a connecting host is neutral and not offline', () => {
    const card = deriveCard(host(), { kind: 'connecting' }, undefined, undefined);
    expect(card.overall).toBe('unknown');
    expect(card.offline).toBe(false);
  });

  it('an unprobed host with no status is neutral and offline', () => {
    const card = deriveCard(host(), undefined, undefined, undefined);
    expect(card.overall).toBe('unknown');
    expect(card.offline).toBe(true);
  });

  it('a single reported metric drives severity while the others stay unknown', () => {
    const card = deriveCard(host(), CONNECTED, metrics({ diskPercent: 90 }), undefined);
    expect(card.overall).toBe('crit');
    expect(card.metricRows).toEqual([
      { label: 'CPU', percent: null, status: 'unknown' },
      { label: 'RAM', percent: null, status: 'unknown' },
      { label: 'Disk', percent: 90, status: 'crit' }
    ]);
  });

  it('carries uptime, os info and top processes through', () => {
    const card = deriveCard(
      host(),
      CONNECTED,
      metrics({ uptime: '3 days', osInfo: 'Ubuntu 22.04', topProcesses: [{ name: 'pg', cpuPercent: 30, memPercent: 12 }] }),
      undefined
    );
    expect(card.uptime).toBe('3 days');
    expect(card.osInfo).toBe('Ubuntu 22.04');
    expect(card.topProcesses).toHaveLength(1);
  });
});

describe('deriveCard — detected services', () => {
  it('names each kind and summarises the docker quick-scan container counts', () => {
    // The quick-scan emits containers_total + containers_running for docker, and no
    // metrics for the other kinds (they render name-only).
    const svc: HostServices = {
      kind: 'detected',
      services: [
        { kind: 'docker', metrics: [{ name: 'containers_total', value: 7 }, { name: 'containers_running', value: 6 }] },
        { kind: 'postgresql', metrics: [] }
      ]
    };
    const card = deriveCard(host(), CONNECTED, undefined, svc);
    expect(card.detectedServices).toEqual([
      { kind: 'docker', name: 'Docker', detail: '6/7 running' },
      { kind: 'postgresql', name: 'PostgreSQL', detail: '' }
    ]);
    expect(card.servicesError).toBeUndefined();
  });

  it('reads a docker host with containers present but none running', () => {
    const svc: HostServices = {
      kind: 'detected',
      services: [{ kind: 'docker', metrics: [{ name: 'containers_total', value: 3 }, { name: 'containers_running', value: 0 }] }]
    };
    expect(deriveCard(host(), CONNECTED, undefined, svc).detectedServices[0].detail).toBe('0/3 running');
  });

  it('shows no docker detail until its quick-scan metrics arrive', () => {
    const svc: HostServices = { kind: 'detected', services: [{ kind: 'docker', metrics: [] }] };
    expect(deriveCard(host(), CONNECTED, undefined, svc).detectedServices[0].detail).toBe('');
  });

  it('surfaces a discovery failure and shows no service chips', () => {
    const svc: HostServices = { kind: 'failed', message: 'scan timed out' };
    const card = deriveCard(host(), CONNECTED, undefined, svc);
    expect(card.detectedServices).toEqual([]);
    expect(card.servicesError).toBe('scan timed out');
  });
});

describe('filterHosts — mirrors the TUI host search', () => {
  const card = (o: Partial<HostDto>) => deriveCard({ ...host(), ...o }, CONNECTED, undefined, undefined);
  const cards = [
    card({ name: 'web-prod', hostname: '10.0.0.1', tags: ['production'], notes: 'billing frontend' }),
    card({ name: 'db-staging', hostname: '10.0.0.2', tags: ['staging', 'db'] }),
    card({ name: 'cache', hostname: '192.168.1.5', tags: [] })
  ];
  const names = (q: string) => filterHosts(cards, q).map((c) => c.host.name);

  it('keeps every card for an empty or whitespace query', () => {
    expect(filterHosts(cards, '')).toHaveLength(3);
    expect(filterHosts(cards, '   ')).toHaveLength(3);
  });

  it('matches name, hostname, tags and notes, case-insensitively', () => {
    expect(names('WEB')).toEqual(['web-prod']);
    expect(names('192.168')).toEqual(['cache']);
    expect(names('staging')).toEqual(['db-staging']);
    expect(names('BILLING')).toEqual(['web-prod']);
  });

  it('returns nothing when no card matches', () => {
    expect(filterHosts(cards, 'nope')).toEqual([]);
  });
});

describe('group-by-tag — mirrors the TUI dashboard', () => {
  const card = (name: string, tags: string[]) =>
    deriveCard({ ...host(name), tags }, CONNECTED, undefined, undefined);
  const names = (cs: { host: HostDto }[]) => cs.map((c) => c.host.name);
  const sections = (cards: ReturnType<typeof card>[]) =>
    groupByTag(cards).map((g) => [g.tag, names(g.cards)]);

  it('puts each card under its first tag only, sections case-insensitive, Untagged last', () => {
    const cards = [
      card('web', ['prod', 'Web']),
      card('lab', []),
      card('db', ['prod']),
      card('dev1', ['dev']),
      card('www', ['Web'])
    ];
    expect(sections(cards)).toEqual([
      ['dev', ['dev1']],
      ['prod', ['web', 'db']],
      ['Web', ['www']],
      [null, ['lab']]
    ]);
  });

  it('skips blank tags and trims the one it uses', () => {
    const cards = [card('a', ['', ' db ']), card('b', ['  ']), card('c', ['db', 'db'])];
    expect(sections(cards)).toEqual([
      ['db', ['a', 'c']],
      [null, ['b']]
    ]);
  });

  it('returns no sections for no cards', () => {
    expect(groupByTag([])).toEqual([]);
  });
});

describe('dashboard sort and drag order', () => {
  const FAILED: ConnectionStatusDto = { kind: 'failed', message: 'refused' };
  const withStatus = (name: string, status: ConnectionStatusDto | undefined) =>
    deriveCard(host(name), status, undefined, undefined);
  const card = (name: string) => withStatus(name, CONNECTED);
  const names = (cs: { host: HostDto }[]) => cs.map((c) => c.host.name);
  // No name repeats here, so each host is keyed by its name alone.
  const keyOf = orderKeys([]);
  const keys = (...ns: string[]) => ns.map((n) => keyOf(host(n)));

  it('custom puts listed hosts first and the rest in config order', () => {
    const cards = [card('a'), card('b'), card('c'), card('d')];
    expect(names(sortCards(cards, 'custom', keys('c', 'gone', 'a'), keyOf))).toEqual(['c', 'a', 'b', 'd']);
    expect(names(sortCards(cards, 'custom', [], keyOf))).toEqual(['a', 'b', 'c', 'd']);
  });

  it('name is case-insensitive and natural', () => {
    const cards = [card('web-10'), card('Web-2'), card('db'), card('web-1')];
    expect(names(sortCards(cards, 'name', [], keyOf))).toEqual(['db', 'web-1', 'Web-2', 'web-10']);
  });

  // The order WebKitGTK broke under a POSIX locale. Node maps C.UTF-8 to en-US, so
  // this pins the order but can't reproduce that default.
  it('name mixes capitals in', () => {
    const cards = ['Web-upper', 'api-eu', 'Zeta', 'bad-auth', 'ALPHA'].map(card);
    expect(names(sortCards(cards, 'name', [], keyOf))).toEqual(['ALPHA', 'api-eu', 'bad-auth', 'Web-upper', 'Zeta']);
  });

  it('status follows the TUI: connected, then not yet known, failed last', () => {
    const cards = [
      withStatus('down', FAILED),
      withStatus('new', undefined),
      withStatus('up', CONNECTED),
      withStatus('dialing', { kind: 'connecting' }),
      withStatus('up2', CONNECTED)
    ];
    expect(names(sortCards(cards, 'status', [], keyOf))).toEqual(['up', 'up2', 'new', 'dialing', 'down']);
  });

  it('moves a card within the whole grid', () => {
    const all = [card('a'), card('b'), card('c'), card('d')];
    expect(moveCard(all, all, 3, 0, keyOf)).toEqual(keys('d', 'a', 'b', 'c'));
    expect(moveCard(all, all, 0, 3, keyOf)).toEqual(keys('b', 'c', 'd', 'a'));
    expect(moveCard(all, all, 1, 2, keyOf)).toEqual(keys('a', 'c', 'b', 'd'));
  });

  it('moves a card within a run, leaving the cards outside it in place', () => {
    const [a, x, b, y, c] = [card('a'), card('x'), card('b'), card('y'), card('c')];
    const all = [a, x, b, y, c];
    expect(moveCard(all, [a, b, c], 2, 0, keyOf)).toEqual(keys('c', 'a', 'x', 'b', 'y'));
    expect(moveCard(all, [a, b, c], 0, 2, keyOf)).toEqual(keys('x', 'b', 'y', 'c', 'a'));
  });

  it('seeds the custom order from the sort on screen', () => {
    const shown = sortCards([card('c'), card('a'), card('b')], 'name', [], keyOf);
    expect(moveCard(shown, shown, 2, 1, keyOf)).toEqual(keys('a', 'c', 'b'));
  });
});

describe('custom order with repeated host names', () => {
  // Only a hand-edited hosts.toml or ~/.ssh/config can repeat a name.
  const at = (name: string, port: number, tags: string[] = []) =>
    deriveCard({ ...host(name), port, tags }, CONNECTED, undefined, undefined);
  const ids = (cs: { host: HostDto }[]) => cs.map((c) => `${c.host.name}:${c.host.port}`);
  const keysOf = (cs: { host: HostDto }[]) => orderKeys(cs.map((c) => c.host));

  it('keys a repeated name by its address too, the same on every load', () => {
    const [a, b, solo] = [at('twin', 22), at('twin', 2201), at('solo', 22)];
    const keyOf = keysOf([a, solo, b]);
    expect(new Set([a, b, solo].map((c) => keyOf(c.host))).size).toBe(3);
    expect(keyOf({ ...b.host })).toBe(keyOf(b.host));
    // A unique name alone names its host, so editing the address keeps its place.
    expect(keyOf({ ...solo.host, hostname: 'moved.example.com' })).toBe(keyOf(solo.host));
  });

  it('keeps the parts of a key apart', () => {
    const hosts = [
      { ...host('a'), user: 'b c' },
      { ...host('a'), user: 'x' },
      { ...host('a b'), user: 'c' },
      { ...host('a b'), user: 'x' }
    ];
    expect(new Set(hosts.map(orderKeys(hosts))).size).toBe(4);
  });

  it('moves one twin before the other', () => {
    const all = [at('twin', 22), at('x', 22), at('twin', 2201)];
    const keyOf = keysOf(all);
    const order = moveCard(all, all, 2, 0, keyOf);
    expect(ids(sortCards(all, 'custom', order, keyOf))).toEqual(['twin:2201', 'twin:22', 'x:22']);
  });

  it('moves one of two entries at the same address before the other', () => {
    const all = [at('twin', 22), at('x', 22), at('twin', 22)];
    const keyOf = keysOf(all);
    const order = moveCard(all, all, 2, 0, keyOf);
    expect(sortCards(all, 'custom', order, keyOf).map((c) => all.indexOf(c))).toEqual([2, 0, 1]);
  });

  it('keeps a host in place when a namesake comes or goes', () => {
    const [web, a, b, twin] = [at('web', 22), at('a', 22), at('b', 22), at('web', 2222)];
    const alone = keysOf([web, a, b]);
    const order = moveCard([a, b, web], [a, b, web], 2, 0, alone);
    const both = [web, a, b, twin];
    // The name-only key can't tell which "web" it meant, so the newcomer joins it.
    expect(ids(sortCards(both, 'custom', order, keysOf(both)))).toEqual(['web:22', 'web:2222', 'a:22', 'b:22']);

    const moved = moveCard(both, both, 3, 0, keysOf(both));
    expect(ids(sortCards([web, a, b], 'custom', moved, alone))).toEqual(['web:22', 'a:22', 'b:22']);
    expect(ids(sortCards([a, b, twin], 'custom', moved, keysOf([a, b, twin])))).toEqual(['web:2222', 'a:22', 'b:22']);
  });

  it.each([2202, 22])('leaves a namesake on port %i in another section in place', (port) => {
    const all = [at('u', 22), at('a', 22, ['db']), at('dup', 22, ['db']), at('dup', port)];
    const keyOf = keysOf(all);
    const untagged = [all[0], all[3]];
    const order = moveCard(all, untagged, 1, 0, keyOf);
    const sections = groupByTag(sortCards(all, 'custom', order, keyOf)).map((g) => [g.tag, ids(g.cards)]);
    expect(sections).toEqual([
      ['db', ['a:22', 'dup:22']],
      [null, [`dup:${port}`, 'u:22']]
    ]);
  });
});

// --- Quick-action dispatch: the card's sh/files buttons use the shared spawn path. ---

async function freshNav() {
  vi.resetModules();
  const { QUICK_ACTIONS: actions } = await import('./serverCard');
  const { spawnSession } = await import('$lib/stores/navigation');
  const { sessions } = await import('$lib/stores/sessions');
  const { activeEntity } = await import('$lib/stores/activeEntity');
  return { actions, spawnSession, sessions, activeEntity };
}

describe('quick actions', () => {
  it('map sh to a terminal and files to an SFTP session', () => {
    expect(QUICK_ACTIONS.map((a) => [a.id, a.kind])).toEqual([
      ['sh', 'terminal'],
      ['files', 'sftp']
    ]);
  });

  it('dispatch through the shared spawn path, appending an active session of the right kind', async () => {
    const { actions, spawnSession, sessions, activeEntity } = await freshNav();
    for (const action of actions) {
      const s = spawnSession(action.kind, 'web-1');
      expect(s.kind).toBe(action.kind);
      expect(get(activeEntity)).toEqual({ kind: 'session', id: s.id });
    }
    expect(get(sessions).map((s) => s.kind)).toEqual(['terminal', 'sftp']);
  });
});

describe('deriveCard — reachability hosts', () => {
  it('shows the probe result instead of metric tiles', () => {
    const card = deriveCard(tcpHost(), { kind: 'connected' }, undefined, undefined);
    expect(card.reachability).toBe('reachable');
    expect(card.metricRows).toEqual([]);
    expect(card.overall).toBe('ok');
    expect(card.offline).toBe(false);
  });

  it('reports a failed probe as unreachable and offline', () => {
    const card = deriveCard(tcpHost(), { kind: 'failed', message: 'refused' }, undefined, undefined);
    expect(card.reachability).toBe('unreachable');
    expect(card.offline).toBe(true);
    expect(card.overall).toBe('off');
  });

  it('is still checking before the first probe answers', () => {
    expect(deriveCard(tcpHost(), { kind: 'connecting' }, undefined, undefined).reachability).toBe('checking');
    expect(deriveCard(tcpHost(), undefined, undefined, undefined).reachability).toBe('checking');
  });

  it('never claims metrics an ssh host would have reported', () => {
    const card = deriveCard(tcpHost(), { kind: 'connected' }, metrics({ cpuPercent: 90 }), undefined);
    expect(card.metricRows).toEqual([]);
    expect(card.uptime).toBeUndefined();
  });

  it('leaves an ssh host on the metric path', () => {
    const card = deriveCard(host(), { kind: 'connected' }, metrics({ cpuPercent: 10 }), undefined);
    expect(card.reachability).toBeUndefined();
    expect(card.metricRows).toHaveLength(3);
  });
});

describe('deriveTunnel — the card\'s tunnel block', () => {
  const forwarded = (): HostDto => ({
    ...host('nas'),
    localForwards: [{ bindPort: 9443, remoteHost: '127.0.0.1', remotePort: 9443 }]
  });

  it('is absent for a host without forwards, whatever its status', () => {
    expect(deriveTunnel(host(), { kind: 'up' })).toBeUndefined();
    expect(deriveCard(host(), CONNECTED, undefined, undefined, { kind: 'up' }).tunnel).toBeUndefined();
  });

  it('reads as off until a status arrives, and after a stop', () => {
    expect(deriveTunnel(forwarded(), undefined)).toMatchObject({ running: false, label: 'Off', dot: 'unknown' });
  });

  it('tells running states from ended ones', () => {
    expect(deriveTunnel(forwarded(), { kind: 'connecting' })).toMatchObject({ running: true, dot: 'unknown' });
    expect(deriveTunnel(forwarded(), { kind: 'up' })).toMatchObject({ running: true, dot: 'ok', label: 'Active' });
    expect(deriveTunnel(forwarded(), { kind: 'retrying', message: 'connection lost' })).toMatchObject({
      running: true,
      dot: 'warn',
      message: 'connection lost'
    });
    expect(deriveTunnel(forwarded(), { kind: 'failed', message: 'port 9443 in use' })).toMatchObject({
      running: false,
      dot: 'off',
      message: 'port 9443 in use'
    });
  });

  it('rides along on a reachability card too — a port check still has SSH credentials', () => {
    const card = deriveCard({ ...forwarded(), monitoring: 'tcpPort' }, CONNECTED, undefined, undefined, { kind: 'up' });
    expect(card.reachability).toBe('reachable');
    expect(card.tunnel?.label).toBe('Active');
  });

  it('labels where a forward listens', () => {
    const listen = (bindAddress: string | undefined, on = false) =>
      forwardListen({ bindAddress, bindPort: 80, remoteHost: 'x', remotePort: 1 }, on);
    expect(listen(undefined)).toBe('localhost:80');
    expect(listen('0.0.0.0')).toBe('0.0.0.0:80');
    expect(listen('')).toBe('*:80');
    expect(listen('::1')).toBe('[::1]:80');
    // A LAN address of this machine is masked on stream; wildcards are not.
    expect(listen('192.168.1.20', true)).not.toContain('192.168.1.20');
    expect(listen('0.0.0.0', true)).toBe('0.0.0.0:80');
  });

  it('masks a target in streamer mode, but not the server\'s own loopback', () => {
    const target = (remoteHost: string, on: boolean) => forwardTarget({ bindPort: 1, remoteHost, remotePort: 5432 }, on);
    expect(target('10.20.30.40', false)).toBe('10.20.30.40:5432');
    expect(target('10.20.30.40', true)).not.toContain('10.20.30.40');
    for (const loopback of ['localhost', '127.0.0.1', '::1']) {
      expect(target(loopback, true)).toContain(loopback);
    }
    expect(target('fe80::1', false)).toBe('[fe80::1]:5432');
  });
});

