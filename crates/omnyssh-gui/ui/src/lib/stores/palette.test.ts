import { beforeEach, describe, expect, it } from 'vitest';
import { get } from 'svelte/store';
import type { ConnectionStatusDto, HostDto } from '$lib/bindings';
import type { Session } from './sessions';
import { palette, paletteItems, paletteSignature, nextIndex, hostStatusDot } from './palette';

function host(name: string, extra: Partial<HostDto> = {}): HostDto {
  return {
    name,
    hostname: `${name}.example.com`,
    user: 'deploy',
    port: 22,
    tags: [],
    source: 'manual',
    hasKey: false,
    monitoring: 'ssh',
    localForwards: [],
    tunnelAutostart: false,
    forwardAgent: false,
    ...extra
  };
}

function session(id: number, hostName: string, kind: Session['kind'] = 'terminal'): Session {
  return { id, kind, hostName, status: 'connecting' };
}

describe('paletteItems — filter & sections', () => {
  const hosts = [host('web-1', { tags: ['prod'] }), host('db-1', { user: 'root' }), host('web-2')];
  const sessions = [session(1, 'web-1', 'terminal'), session(2, 'db-1', 'sftp')];

  it('navigator lists sessions first, then hosts', () => {
    const items = paletteItems('navigate', hosts, sessions, '');
    expect(items.map((i) => i.kind)).toEqual(['session', 'session', 'host', 'host', 'host']);
  });

  it('picker lists only hosts', () => {
    const items = paletteItems('pickHost', hosts, sessions, '');
    expect(items.every((i) => i.kind === 'host')).toBe(true);
    expect(items).toHaveLength(3);
  });

  it('filters hosts by name, hostname, user and tags', () => {
    expect(paletteItems('pickHost', hosts, [], 'web').map((i) => i.kind === 'host' && i.host.name))
      .toEqual(['web-1', 'web-2']);
    expect(paletteItems('pickHost', hosts, [], 'root')).toHaveLength(1); // db-1 by user
    expect(paletteItems('pickHost', hosts, [], 'prod')).toHaveLength(1); // web-1 by tag
    expect(paletteItems('pickHost', hosts, [], 'example.com')).toHaveLength(3); // hostname
  });

  it('requires every whitespace-separated token to match (AND)', () => {
    expect(paletteItems('pickHost', hosts, [], 'web prod').map((i) => i.kind === 'host' && i.host.name))
      .toEqual(['web-1']);
    expect(paletteItems('pickHost', hosts, [], 'web root')).toHaveLength(0);
  });

  it('filters sessions by host name and kind in the navigator', () => {
    expect(paletteItems('navigate', [], sessions, 'sftp').map((i) => i.kind === 'session' && i.session.id))
      .toEqual([2]);
    expect(paletteItems('navigate', [], sessions, 'web')).toHaveLength(1);
  });

  it('an empty query keeps everything', () => {
    expect(paletteItems('navigate', hosts, sessions, '   ')).toHaveLength(5);
  });

  it('lists a shared host name once, as its first host, whatever the query', () => {
    const twins = [host('web-1'), host('web-1', { hostname: 'twin.example.com' })];
    expect(paletteItems('pickHost', twins, [], '')).toEqual([{ kind: 'host', host: twins[0] }]);
    expect(paletteItems('navigate', twins, [], 'twin')).toHaveLength(0);
  });
});

describe('paletteSignature — stable across volatile updates', () => {
  it('is identical when only a session status changes (no spurious highlight reset)', () => {
    const connecting = paletteItems('navigate', [host('web-1')], [session(1, 'web-1')], '');
    const connected = paletteItems(
      'navigate',
      [host('web-1')],
      [{ ...session(1, 'web-1'), status: 'connected' }],
      ''
    );
    expect(paletteSignature(connecting)).toBe(paletteSignature(connected));
  });

  it('changes when a row is added, removed, or reordered', () => {
    const base = paletteItems('navigate', [host('web-1'), host('web-2')], [], '');
    expect(paletteSignature(base)).not.toBe(
      paletteSignature(paletteItems('navigate', [host('web-1')], [], ''))
    );
    expect(paletteSignature(base)).not.toBe(
      paletteSignature(paletteItems('navigate', [host('web-2'), host('web-1')], [], ''))
    );
  });

  it('changes when a session row appears', () => {
    const withoutSession = paletteItems('navigate', [host('web-1')], [], '');
    const withSession = paletteItems('navigate', [host('web-1')], [session(1, 'web-1')], '');
    expect(paletteSignature(withoutSession)).not.toBe(paletteSignature(withSession));
  });
});

describe('nextIndex — wrapping selection', () => {
  it('moves forward and wraps past the end', () => {
    expect(nextIndex(0, 1, 3)).toBe(1);
    expect(nextIndex(2, 1, 3)).toBe(0);
  });

  it('moves back and wraps past the start', () => {
    expect(nextIndex(0, -1, 3)).toBe(2);
    expect(nextIndex(1, -1, 3)).toBe(0);
  });

  it('stays at 0 for an empty list', () => {
    expect(nextIndex(0, 1, 0)).toBe(0);
    expect(nextIndex(0, -1, 0)).toBe(0);
  });
});

describe('hostStatusDot — connection state → dot', () => {
  const dot = (status: ConnectionStatusDto | undefined) => hostStatusDot(status);
  it('connected is ok, failed is offline, everything else neutral', () => {
    expect(dot({ kind: 'connected' })).toBe('ok');
    expect(dot({ kind: 'failed', message: 'x' })).toBe('off');
    expect(dot({ kind: 'connecting' })).toBe('unknown');
    expect(dot({ kind: 'unknown' })).toBe('unknown');
    expect(dot(undefined)).toBe('unknown');
  });
});

describe('palette store — modes & picker resolution', () => {
  beforeEach(() => palette.close());

  it('open() shows the navigator', () => {
    palette.open();
    expect(get(palette)).toEqual({ open: true, mode: 'navigate' });
  });

  it('pickHost() resolves with the chosen host and closes', async () => {
    const pending = palette.pickHost();
    expect(get(palette)).toEqual({ open: true, mode: 'pickHost' });
    const chosen = host('web-1');
    palette.choose(chosen);
    await expect(pending).resolves.toEqual(chosen);
    expect(get(palette)).toEqual({ open: false, mode: 'navigate' });
  });

  it('closing a pending picker resolves null', async () => {
    const pending = palette.pickHost();
    palette.close();
    await expect(pending).resolves.toBeNull();
  });

  it('opening the navigator cancels a pending picker (resolves null)', async () => {
    const pending = palette.pickHost();
    palette.open();
    await expect(pending).resolves.toBeNull();
    expect(get(palette)).toEqual({ open: true, mode: 'navigate' });
  });
});
