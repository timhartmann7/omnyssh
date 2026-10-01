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
  formatBytes,
  rootOf,
  type Pane,
  type SftpSession,
} from "./sftp";

function entry(name: string, isDir = false, size = 0): FileEntryDto {
  return { name, path: `/srv/${name}`, size, isDir };
}

function paneWith(entries: FileEntryDto[], marked: string[] = []): Pane {
  return { path: "/srv", entries, loading: false, marked: new Set(marked) };
}

describe("sftp reducers", () => {
  it("starts a session connecting with both panes empty and loading", () => {
    const s = newSession("web-1");
    expect(s.status).toBe("connecting");
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

  it("correlates a two-file batch by FIFO order", () => {
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

  it("formatBytes is human readable", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
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

describe("sftp store", () => {
  it("keeps concurrent sessions isolated and prunes on close", () => {
    sftp.open(1, "web-1");
    sftp.open(2, "db-1");

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
