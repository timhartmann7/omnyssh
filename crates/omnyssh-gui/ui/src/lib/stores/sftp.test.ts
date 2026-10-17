import { describe, expect, it } from "vitest";
import { get } from "svelte/store";
import type { FileEntryDto } from "$lib/bindings";
import {
  sftp,
  newSession,
  applyListing,
  toggleMark,
  markedEntries,
  mergeRefresh,
  applyProgress,
  applyOpDone,
  transferState,
  formatBytes,
  formatDate,
  dragPayload,
  clashCount,
  repeatedName,
  rootOf,
  baseName,
  isPlainName,
  CANCELLED,
  type Pane,
  type SftpSession,
} from "./sftp";

function entry(name: string, isDir = false, size = 0): FileEntryDto {
  return { name, path: `/srv/${name}`, size, isDir, modified: null };
}

function paneWith(entries: FileEntryDto[], marked: string[] = []): Pane {
  return { path: "/srv", entries, loading: false, marked: new Set(marked) };
}

describe('formatDate', () => {
  it('renders unknown times as an em dash', () => {
    expect(formatDate(null)).toBe('—');
    expect(formatDate(undefined)).toBe('—');
    expect(formatDate(Number.NaN)).toBe('—');
  });

  it('renders a known time with its year, short and long', () => {
    const secs = Date.UTC(2024, 5, 15, 12, 0) / 1000;
    expect(formatDate(secs)).toContain('2024');
    expect(formatDate(secs, true)).toContain('2024');
    expect(formatDate(secs, true).length).toBeGreaterThan(formatDate(secs).length);
  });
});

describe('dragPayload', () => {
  const a = entry('a.txt');
  const b = entry('b.txt');
  const dir = entry('logs', true);
  const parent = { ...entry('..', true), path: '/' };

  it('carries only the dragged file when it is not marked', () => {
    expect(dragPayload(paneWith([a, b], [b.path]), a)).toEqual([a]);
  });

  it('carries every marked entry, folders included, in listing order', () => {
    const pane = paneWith([a, dir, b], [b.path, dir.path, a.path]);
    expect(dragPayload(pane, b)).toEqual([a, dir, b]);
    expect(dragPayload(pane, dir)).toEqual([a, dir, b]);
  });

  it('carries an unmarked folder on its own', () => {
    expect(dragPayload(paneWith([a, dir], [a.path]), dir)).toEqual([dir]);
  });

  it('carries nothing for the parent row', () => {
    expect(dragPayload(paneWith([parent]), parent)).toEqual([]);
  });
});

describe('clashCount', () => {
  const listing = [{ ...entry('..', true), path: '/' }, entry('App.log'), entry('www', true)];

  it('counts the names the listing already has, files and folders alike', () => {
    expect(clashCount(['www', 'App.log', 'new.txt'], listing, false)).toBe(2);
    expect(clashCount(['new.txt'], listing, false)).toBe(0);
  });

  it('never counts the parent row', () => {
    expect(clashCount(['..'], listing, false)).toBe(0);
  });

  it('matches across case only where the file system ignores it', () => {
    expect(clashCount(['app.log', 'WWW'], listing, false)).toBe(0);
    expect(clashCount(['app.log', 'WWW'], listing, true)).toBe(2);
  });

  it('matches an accented letter composed or not where case is ignored, as macOS does', () => {
    const decomposed = [entry('cafe\u0301.txt')];
    expect(clashCount(['caf\u00e9.txt'], decomposed, true)).toBe(1);
    expect(clashCount(['caf\u00e9.txt'], decomposed, false)).toBe(0);
  });
});

describe('repeatedName', () => {
  it('finds two items that would land on one name', () => {
    expect(repeatedName(['readme.txt', 'a.png', 'readme.txt'], false)).toBe('readme.txt');
    expect(repeatedName(['readme.txt', 'a.png'], false)).toBeUndefined();
  });

  it('matches across case only where the file system ignores it', () => {
    expect(repeatedName(['A.txt', 'a.txt'], false)).toBeUndefined();
    expect(repeatedName(['A.txt', 'a.txt'], true)).toBe('a.txt');
  });
});

describe('isPlainName', () => {
  it('takes one plain component and nothing that would land elsewhere', () => {
    for (const name of ['notes.txt', '..hidden', 'a b', 'a\\b']) {
      expect(isPlainName(name, false)).toBe(true);
    }
    for (const name of ['', '.', '..', 'a/b', './x', 'x/', '.config/autostart']) {
      expect(isPlainName(name, false)).toBe(false);
    }
  });

  it('splits on a backslash only on Windows', () => {
    expect(isPlainName('a\\b', true)).toBe(false);
    expect(isPlainName('..\\x', true)).toBe(false);
    expect(isPlainName('notes.txt', true)).toBe(true);
  });
});

describe('sftp reducers', () => {
  it('starts a session connecting with both panes empty and loading', () => {
    const s = newSession('web-1');
    expect(s.status).toBe('connecting');
    expect(s.local.loading).toBe(true);
    expect(s.remote.loading).toBe(true);
    expect(s.local.entries).toEqual([]);
    expect(s.pending).toEqual([]);
  });

  it("applyListing replaces entries at a path and clears marks + loading", () => {
    const pane = paneWith([entry("a")], ["/srv/a"]);
    const next = applyListing(pane, "/etc", [entry("b"), entry("c")]);
    expect(next.path).toBe("/etc");
    expect(next.entries.map((e) => e.name)).toEqual(["b", "c"]);
    expect(next.loading).toBe(false);
    expect(next.marked.size).toBe(0);
  });

  it("toggleMark marks and unmarks, and markedEntries keeps listing order", () => {
    let pane = paneWith([entry("a"), entry("b"), entry("c")]);
    pane = toggleMark(pane, "/srv/c");
    pane = toggleMark(pane, "/srv/a");

    expect(markedEntries(pane).map((e) => e.name)).toEqual(["a", "c"]);

    pane = toggleMark(pane, "/srv/a");
    expect(markedEntries(pane).map((e) => e.name)).toEqual(["c"]);
  });

  it("mergeRefresh widens two different sides to both", () => {
    expect(mergeRefresh(undefined, "remote")).toBe("remote");
    expect(mergeRefresh("remote", "remote")).toBe("remote");
    expect(mergeRefresh("local", "remote")).toBe("both");
    expect(mergeRefresh("both", "local")).toBe("both");
  });

  it("applyProgress binds a tick to the front pending transfer op", () => {
    const s: SftpSession = {
      ...newSession("web-1"),
      pending: [{ kind: "upload", name: "a.txt", refresh: "remote" }],
    };

    const next = applyProgress(s, {
      sessionId: 1,
      transferId: 9,
      rootName: "folder",
      currentFile: "a.txt",
      stage: "transferring",
      bytesDone: 50,
      bytesTotal: 100,
      filesDone: 1,
      filesTotal: 1,
    });

    expect(next.transfer).toEqual({
      transferId: 9,
      kind: "upload",
      rootName: "folder",
      currentFile: "a.txt",
      stage: "transferring",
      bytesDone: 50,
      bytesTotal: 100,
      filesDone: 1,
      filesTotal: 1,
    });
  });

  it("applyProgress accepts delete progress and exposes item counts", () => {
    const s: SftpSession = {
      ...newSession("web-1"),
      pending: [{ kind: "delete", name: "logs", refresh: "remote" }],
    };

    const next = applyProgress(s, {
      sessionId: 1,
      transferId: 12,
      rootName: "logs",
      currentFile: "old.log",
      stage: "transferring",
      bytesDone: 3,
      bytesTotal: 10,
      filesDone: 3,
      filesTotal: 10,
    });

    expect(next.transfer?.kind).toBe("delete");
    expect(next.transfer?.filesDone).toBe(3);
    expect(next.transfer?.filesTotal).toBe(10);
  });

  it("applyProgress ignores a tick when the front op is not a transfer", () => {
    const s: SftpSession = {
      ...newSession("web-1"),
      pending: [{ kind: "mkdir", refresh: "remote" }],
    };

    expect(
      applyProgress(s, {
        sessionId: 1,
        transferId: 9,
        rootName: "",
        currentFile: "",
        stage: "transferring",
        bytesDone: 1,
        bytesTotal: 2,
        filesDone: 1,
        filesTotal: 1,
      }).transfer,
    ).toBeUndefined();
  });

  it("applyOpDone pops the front op (FIFO), records its refresh, and clears a transfer", () => {
    const s: SftpSession = {
      ...newSession("web-1"),
      pending: [
        { kind: "upload", name: "a", refresh: "remote" },
        { kind: "mkdir", refresh: "remote" },
      ],
      transfer: {
        transferId: 9,
        kind: "upload",
        rootName: "folder",
        currentFile: "a",
        stage: "transferring",
        bytesDone: 100,
        bytesTotal: 100,
        filesDone: 1,
        filesTotal: 1,
      },
    };

    const next = applyOpDone(s, true);

    expect(next.pending.map((p) => p.kind)).toEqual(["mkdir"]);
    expect(next.refresh).toBe("remote");
    expect(next.transfer).toBeUndefined();
    expect(next.error).toBeUndefined();
  });

  it("applyOpDone clears delete progress on completion", () => {
    const s: SftpSession = {
      ...newSession("web-1"),
      pending: [{ kind: "delete", name: "logs", refresh: "remote" }],
      transfer: {
        transferId: 12,
        kind: "delete",
        rootName: "logs",
        currentFile: "old.log",
        stage: "transferring",
        bytesDone: 10,
        bytesTotal: 10,
        filesDone: 10,
        filesTotal: 10,
      },
    };

    expect(applyOpDone(s, true).transfer).toBeUndefined();
  });

  it("applyOpDone surfaces the error message on failure", () => {
    const s: SftpSession = {
      ...newSession("web-1"),
      pending: [{ kind: "delete", name: "x", refresh: "remote" }],
    };

    const next = applyOpDone(s, false, "permission denied");

    expect(next.error).toBe("permission denied");
    expect(next.pending).toEqual([]);
  });

  it("applyOpDone keeps a prior op error on a later success", () => {
    let s: SftpSession = {
      ...newSession("web-1"),
      pending: [
        { kind: "delete", name: "logs", refresh: "remote" },
        { kind: "delete", name: "notes.txt", refresh: "remote" },
      ],
    };

    s = applyOpDone(s, false, "directory not empty");
    expect(s.error).toBe("directory not empty");

    s = applyOpDone(s, true);
    expect(s.error).toBe("directory not empty");
    expect(s.pending).toEqual([]);
  });

  it('applyOpDone keeps a prior op error when the rest of the batch is cancelled', () => {
    const upload = (name: string) => ({ kind: 'upload' as const, name, refresh: 'remote' as const });
    let s: SftpSession = { ...newSession('web-1'), pending: [upload('a.txt'), upload('b.txt')] };
    s = applyOpDone(s, false, "open 'a.txt': Permission denied");
    s = applyOpDone(s, false, CANCELLED);
    expect(s.error).toBe("open 'a.txt': Permission denied");

    // A cancel on its own still says so.
    s = applyOpDone({ ...newSession('web-1'), pending: [upload('c.txt')] }, false, CANCELLED);
    expect(s.error).toBe(CANCELLED);
  });

  it('correlates a two-file batch by FIFO order across progress + op-done', () => {
    // The core is sequential, so the front pending op is always the one running: A's
    // progress shows A; A's op-done pops it; then B's progress shows B (§3.2/§4.3).
    let s: SftpSession = {
      ...newSession("web-1"),
      pending: [
        { kind: "upload", name: "A", refresh: "remote" },
        { kind: "upload", name: "B", refresh: "remote" },
      ],
    };

    s = applyProgress(s, {
      sessionId: 1,
      transferId: 1,
      rootName: "folder",
      currentFile: "A",
      stage: "transferring",
      bytesDone: 5,
      bytesTotal: 10,
      filesDone: 1,
      filesTotal: 2,
    });

    expect(s.transfer?.currentFile).toBe("A");

    s = applyOpDone(s, true);
    expect(s.transfer).toBeUndefined();

    s = applyProgress(s, {
      sessionId: 1,
      transferId: 2,
      rootName: "folder",
      currentFile: "B",
      stage: "transferring",
      bytesDone: 10,
      bytesTotal: 10,
      filesDone: 2,
      filesTotal: 2,
    });

    expect(s.transfer?.currentFile).toBe("B");

    s = applyOpDone(s, true);

    expect(s.pending).toEqual([]);
    expect(s.refresh).toBe("remote");
  });

  it('transferState shows a dispatched transfer as preparing until its first tick', () => {
    const idle = newSession('web-1');
    expect(transferState(idle)).toBeUndefined();
    const mkdir: SftpSession = { ...idle, pending: [{ kind: 'mkdir', refresh: 'remote' }] };
    expect(transferState(mkdir)).toBeUndefined();

    let s: SftpSession = {
      ...idle,
      pending: [{ kind: 'download', name: 'logs', refresh: 'local' }]
    };
    expect(transferState(s)).toMatchObject({
      kind: 'download',
      name: 'logs',
      done: 0,
      total: 0,
      preparing: true
    });
    // A folder of empty files still ticks once planned: 0 of 0, no longer preparing.
    s = applyProgress(s, {
      sessionId: 1,
      transferId: 3,
      rootName: 'logs',
      currentFile: '',
      stage: 'transferring',
      bytesDone: 0,
      bytesTotal: 0,
      filesDone: 0,
      filesTotal: 0,
    });
    expect(transferState(s)).toMatchObject({
      kind: 'download',
      name: 'logs',
      done: 0,
      total: 0,
      preparing: false
    });
    expect(transferState(applyOpDone(s, true))).toBeUndefined();
  });

  it('formatBytes is human readable', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });
});

describe("rootOf", () => {
  const drives = ["C:\\", "D:\\"];

  it("finds the drive a local path is on, whatever the letter case", () => {
    expect(rootOf("C:\\Users\\me", drives)).toBe("C:\\");
    expect(rootOf("d:\\Media", drives)).toBe("D:\\");
    expect(rootOf("D:\\", drives)).toBe("D:\\");
  });

  it("has no root for a network share or an unknown drive", () => {
    expect(rootOf("\\\\nas\\share", drives)).toBe("");
    expect(rootOf("E:\\x", drives)).toBe("");
  });

  it("puts every path under / on a single-root system", () => {
    expect(rootOf("/home/me", ["/"])).toBe("/");
  });
});

describe('baseName', () => {
  it('takes the last component of a dropped path', () => {
    expect(baseName('/tmp/album/')).toBe('album');
    expect(baseName('/home/me/back\\slash.txt')).toBe('back\\slash.txt');
    expect(baseName('C:\\Users\\me\\photo.png')).toBe('photo.png');
  });

  it('has no name for a root', () => {
    expect(baseName('/')).toBe('');
    expect(baseName('C:\\')).toBe('');
    expect(baseName('d:')).toBe('');
    expect(baseName('\\\\server\\share')).toBe('');
    expect(baseName('\\\\server\\share\\')).toBe('');
    expect(baseName('\\\\?\\C:\\')).toBe('');
    expect(baseName('\\\\?\\Volume{0b1c}\\')).toBe('');
    expect(baseName('\\\\?\\UNC\\server\\share\\')).toBe('');
  });

  it('names what lies on a share or behind a long path', () => {
    expect(baseName('\\\\server\\share\\album')).toBe('album');
    expect(baseName('\\\\?\\C:\\Users\\me\\photo.png')).toBe('photo.png');
    expect(baseName('\\\\?\\UNC\\server\\share\\album\\')).toBe('album');
  });
});

describe('sftp store', () => {
  it('keeps concurrent sessions isolated and prunes on close', () => {
    sftp.open(1, 'web-1');
    sftp.open(2, 'db-1');
    sftp.listing(1, 'remote', '/a', [entry('one')]);
    sftp.listing(2, 'remote', '/b', [entry('two'), entry('three')]);

    sftp.listing(1, "remote", "/a", [entry("one")]);
    sftp.listing(2, "remote", "/b", [entry("two"), entry("three")]);

    expect(get(sftp).get(1)?.remote.path).toBe("/a");
    expect(get(sftp).get(1)?.remote.entries).toHaveLength(1);

    expect(get(sftp).get(2)?.remote.path).toBe("/b");
    expect(get(sftp).get(2)?.remote.entries).toHaveLength(2);

    sftp.remove(1);
    expect(get(sftp).has(1)).toBe(false);
    expect(get(sftp).has(2)).toBe(true);

    sftp.remove(2);
  });

  it("ignores mutations targeting an unknown session", () => {
    sftp.listing(999, "remote", "/gone", [entry("x")]);
    expect(get(sftp).has(999)).toBe(false);
  });

  it("clearError drops a lingering batch error", () => {
    sftp.open(1, "web-1");
    sftp.pushOp(1, { kind: "delete", name: "logs", refresh: "remote" });

    sftp.opDone(1, false, "directory not empty");
    expect(get(sftp).get(1)?.error).toBe("directory not empty");

    sftp.clearError(1);
    expect(get(sftp).get(1)?.error).toBeUndefined();

    sftp.remove(1);
  });

  it("sessionError clears only the remote loading state", () => {
    sftp.open(1, "web-1");

    sftp.beginLoading(1, "remote");
    sftp.beginLoading(1, "local");

    sftp.sessionError(1, "ListDir failed");

    const s = get(sftp).get(1);

    expect(s?.error).toBe("ListDir failed");
    expect(s?.remote.loading).toBe(false);
    expect(s?.local.loading).toBe(true);

    sftp.remove(1);
  });
});
