// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import type { CloseRequestedEvent, Window } from '@tauri-apps/api/window';
import type { TunnelStatusDto } from '$lib/bindings';
import type { Session } from './sessions';
import { newSession, type SftpSession } from './sftp';
import { tunnels } from './tunnels';
import { beginRun, clearRun, snippetRun } from './snippets';
import { countLiveWork, describeLiveWork, handleCloseRequested, quitPrompt, type LiveWork } from './quit';

const NONE: LiveWork = { terminals: 0, transfers: 0, tunnels: 0, snippetRuns: 0 };

const terminal = (status: Session['status']): Session => ({
  id: Math.random(),
  kind: 'terminal',
  hostName: 'web-1',
  status
});

describe('countLiveWork', () => {
  it('counts only what a quit would cut off', () => {
    const list = [
      terminal('connecting'),
      terminal('connected'),
      // An ended tab is only its last screen; a failed one never connected.
      terminal('closed'),
      terminal('failed'),
      { ...terminal('connected'), kind: 'sftp' } as Session
    ];
    const idle = newSession('web-1');
    const running: SftpSession = {
      ...newSession('db-1'),
      transfer: {
        transferId: 1,
        stage: 'transferring',
        kind: 'download',
        rootName: 'a.log',
        currentFile: 'a.log',
        bytesDone: 1,
        bytesTotal: 2,
        filesDone: 1,
        filesTotal: 1
      }
    };
    // Queued behind a mkdir: not running yet, but it would be lost all the same.
    const queued: SftpSession = {
      ...newSession('db-2'),
      pending: [
        { kind: 'mkdir', refresh: 'remote' },
        { kind: 'upload', name: 'b.txt', refresh: 'remote' }
      ]
    };
    const busyOnly: SftpSession = { ...newSession('db-3'), pending: [{ kind: 'delete', refresh: 'remote' }] };
    const tunnels = new Map<string, TunnelStatusDto>([
      ['a', { kind: 'connecting' }],
      ['b', { kind: 'up' }],
      ['c', { kind: 'retrying', message: 'refused' }],
      ['d', { kind: 'failed', message: 'gone' }]
    ]);
    const run = {
      snippetName: 'deploy',
      entries: [
        { hostName: 'a', pending: false, ok: true, output: '' },
        { hostName: 'b', pending: true, ok: false, output: '' }
      ]
    };

    expect(
      countLiveWork(list, new Map([[1, idle], [2, running], [3, queued], [4, busyOnly]]), tunnels, run)
    ).toEqual({ terminals: 2, transfers: 2, tunnels: 3, snippetRuns: 1 });
  });

  it('finds nothing live in an idle app or a finished snippet run', () => {
    const done = { snippetName: 'x', entries: [{ hostName: 'a', pending: false, ok: true, output: '' }] };
    expect(countLiveWork([], new Map(), new Map(), null)).toEqual(NONE);
    expect(countLiveWork([], new Map(), new Map(), done)).toEqual(NONE);
  });

  it('still counts a snippet run whose results panel was closed', () => {
    beginRun('deploy', ['a']);
    clearRun();
    expect(countLiveWork([], new Map(), new Map(), get(snippetRun)).snippetRuns).toBe(1);
    snippetRun.set(null);
  });
});

describe('handleCloseRequested', () => {
  const order: string[] = [];
  const event = { preventDefault: vi.fn(() => order.push('preventDefault')) };
  const win = {
    isVisible: vi.fn(async () => true),
    unminimize: vi.fn(async () => void order.push('unminimize')),
    setFocus: vi.fn(async () => void order.push('setFocus'))
  };
  const close = () =>
    handleCloseRequested(event as unknown as CloseRequestedEvent, win as unknown as Window);

  beforeEach(() => {
    order.length = 0;
    vi.clearAllMocks();
    win.isVisible.mockResolvedValue(true);
    quitPrompt.set(null);
    tunnels.set(new Map());
  });

  it('asks in a window brought out of the taskbar or dock, after holding the close', async () => {
    tunnels.set(new Map([['web-1', { kind: 'up' }]]));
    await close();
    expect(get(quitPrompt)).toBe('1 tunnel');
    expect(order).toEqual(['preventDefault', 'unminimize', 'setFocus']);
  });

  it('lets the window close when nothing is live', async () => {
    await close();
    expect(order).toEqual([]);
    expect(get(quitPrompt)).toBeNull();
  });

  it('keeps a window the tray hid, without asking or showing it', async () => {
    tunnels.set(new Map([['web-1', { kind: 'up' }]]));
    win.isVisible.mockResolvedValue(false);
    await close();
    expect(order).toEqual(['preventDefault']);
    expect(get(quitPrompt)).toBeNull();
  });
});

describe('describeLiveWork', () => {
  it('lists the counts, singular or plural, with "and" before the last', () => {
    expect(describeLiveWork({ terminals: 2, transfers: 1, tunnels: 1, snippetRuns: 0 })).toBe(
      '2 terminals, 1 transfer and 1 tunnel'
    );
    expect(describeLiveWork({ ...NONE, terminals: 1 })).toBe('1 terminal');
    expect(describeLiveWork({ ...NONE, tunnels: 3, snippetRuns: 1 })).toBe('3 tunnels and 1 snippet run');
    expect(describeLiveWork({ terminals: 1, transfers: 2, tunnels: 2, snippetRuns: 1 })).toBe(
      '1 terminal, 2 transfers, 2 tunnels and 1 snippet run'
    );
  });

  it('is empty when nothing is live', () => {
    expect(describeLiveWork(NONE)).toBe('');
  });
});

// Persisted like the other UI prefs: a fake Tauri store, a fresh module per test.
const backend = { get: vi.fn(), set: vi.fn(), save: vi.fn() };
vi.mock('@tauri-apps/plugin-store', () => ({ load: vi.fn(async () => backend) }));

async function fresh() {
  vi.resetModules();
  return (await import('./quit')).confirmQuit;
}

describe('confirmQuit persistence', () => {
  beforeEach(() => {
    localStorage.clear();
    backend.get.mockReset();
    backend.set.mockReset().mockResolvedValue(undefined);
    backend.save.mockReset().mockResolvedValue(undefined);
  });

  it('defaults to asking and reads the localStorage mirror', async () => {
    expect(get(await fresh())).toBe(true);
    localStorage.setItem('omnyssh-confirm-quit', 'false');
    expect(get(await fresh())).toBe(false);
  });

  it('has the store written by the time set resolves, as a quit follows at once', async () => {
    const confirmQuit = await fresh();
    await confirmQuit.set(false);
    expect(backend.set).toHaveBeenCalledWith('confirmQuit', false);
    expect(backend.save).toHaveBeenCalled();
    expect(localStorage.getItem('omnyssh-confirm-quit')).toBe('false');
  });

  it('hydrate applies the stored value without clobbering a fresh user flip', async () => {
    backend.get.mockResolvedValue(false);
    const confirmQuit = await fresh();
    void confirmQuit.set(true);
    await confirmQuit.hydrate();
    expect(get(confirmQuit)).toBe(true);
  });
});
