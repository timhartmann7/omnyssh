// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { maskHostname, displayHostname, maskUser, displayUser, displayHostTitle } from './streamer';

// Streamer mode disguises host addresses on screen (tech-gui.md §4.3). The mask must be
// deterministic (same host → same disguise all recording), never leak the real value,
// and keep the address shape so cards still read like real servers.
describe('address masking', () => {
  it('is deterministic for a given host', () => {
    expect(maskHostname('203.0.113.7')).toBe('127.0.0.1');
    expect(maskHostname('db.example.com')).toBe('127.0.0.1');
  });

  it('maps an IPv4 to 127.0.0.1', () => {
    expect(maskHostname('192.168.1.10')).toBe('127.0.0.1');
    expect(maskHostname('47.100.20.30')).toBe('127.0.0.1');
  });

  it('maps an IPv6 to ::1', () => {
    expect(maskHostname('2001:db8::1')).toBe('::1');
  });

  it('displayHostname passes through when streamer mode is off', () => {
    expect(displayHostname('203.0.113.7', false)).toBe('203.0.113.7');
    expect(displayHostname('203.0.113.7', true)).toBe('127.0.0.1');
  });

  it('masks username to admin in streamer mode', () => {
    expect(maskUser('root')).toBe('admin');
    expect(maskUser('ubuntu')).toBe('admin');
    expect(displayUser('root', false)).toBe('root');
    expect(displayUser('root', true)).toBe('admin');
  });

  it('disguises host title when matching IP or hostname, preserves custom aliases', () => {
    expect(displayHostTitle('47.100.20.30', false)).toBe('47.100.20.30');
    expect(displayHostTitle('47.100.20.30', true)).toBe('127.0.0.1');
    expect(displayHostTitle('my-host', true, 'my-host')).toBe('127.0.0.1');
    expect(displayHostTitle('生产服务器', true, '47.100.20.30')).toBe('生产服务器');
  });
});

// Persistence mirrors the sidebar-collapse pref: a fake Tauri store, a fresh module per
// test to reset the singleton, and the localStorage mirror seeding the initial value.
const backend = { get: vi.fn(), set: vi.fn(), save: vi.fn() };
vi.mock('@tauri-apps/plugin-store', () => ({ load: vi.fn(async () => backend) }));

async function fresh() {
  vi.resetModules();
  return (await import('./streamer')).streamerMode;
}

describe('streamer mode persistence', () => {
  beforeEach(() => {
    localStorage.clear();
    backend.get.mockReset();
    backend.set.mockReset().mockResolvedValue(undefined);
    backend.save.mockReset().mockResolvedValue(undefined);
  });

  it('defaults to off and mirrors a toggle to localStorage', async () => {
    const streamerMode = await fresh();
    expect(get(streamerMode)).toBe(false);
    streamerMode.toggle();
    expect(get(streamerMode)).toBe(true);
    expect(localStorage.getItem('omnyssh-streamer-mode')).toBe('true');
  });

  it('writes the canonical tauri-plugin-store on a user flip', async () => {
    const streamerMode = await fresh();
    streamerMode.set(true);
    await vi.waitFor(() => {
      expect(backend.set).toHaveBeenCalledWith('streamerMode', true);
      expect(backend.save).toHaveBeenCalled();
    });
  });

  it('hydrate applies the stored value without clobbering a fresh user flip', async () => {
    backend.get.mockResolvedValue(false); // stale persisted value
    const streamerMode = await fresh();
    streamerMode.set(true); // user acts before hydrate resolves
    await streamerMode.hydrate();
    expect(get(streamerMode)).toBe(true);
  });
});
