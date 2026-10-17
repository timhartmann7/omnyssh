import { writable } from "svelte/store";
import { commands } from "$lib/bindings";

export type EditorSource =
  { kind: "remote"; sessionId: number } | { kind: "local" };

export interface EditorTab {
  source: EditorSource;
  path: string;
  name: string;
  content: string;
  original: string;
  loading: boolean;
  saving: boolean;
  dirty: boolean;
  error?: string;
}

const sameTab = (tab: EditorTab, source: EditorSource, path: string) => {
  if (tab.path !== path) return false;

  if (source.kind === "local") {
    return tab.source.kind === "local";
  }

  return (
    tab.source.kind === "remote" && tab.source.sessionId === source.sessionId
  );
};

function createFileEditors() {
  const { subscribe, update } = writable<EditorTab[]>([]);

  const openInternal = (source: EditorSource, path: string) => {
    update((tabs) => {
      if (tabs.some((t) => sameTab(t, source, path))) return tabs;

      return [
        ...tabs,
        {
          source,
          path,
          name: path.split(/[\\/]/).pop() ?? path,
          content: "",
          original: "",
          loading: true,
          saving: false,
          dirty: false,
        },
      ];
    });
  };

  const mutate = (
    source: EditorSource,
    path: string,
    fn: (tab: EditorTab) => EditorTab,
  ) => {
    update((tabs) => tabs.map((t) => (sameTab(t, source, path) ? fn(t) : t)));
  };

  return {
    subscribe,

    async open(sessionId: number, path: string) {
      const source: EditorSource = { kind: "remote", sessionId };

      openInternal(source, path);

      const r = await commands.sftpReadFile(sessionId, path);

      if (r.status === "error") {
        this.error(source, path, r.error.message);
      }
    },

    async openLocal(path: string) {
      const source: EditorSource = { kind: "local" };

      openInternal(source, path);

      const r = await commands.localReadFile(path);

      if (r.status === "error") {
        this.error(source, path, r.error.message);
        return;
      }

      this.setContent(source, path, r.data);
    },

    close(source: EditorSource, path: string) {
      update((tabs) => tabs.filter((t) => !sameTab(t, source, path)));
    },

    setContent(source: EditorSource, path: string, content: string) {
      mutate(source, path, (t) => ({
        ...t,
        content,
        original: content,
        loading: false,
        dirty: false,
        error: undefined,
      }));
    },

    edit(source: EditorSource, path: string, content: string) {
      mutate(source, path, (t) => ({
        ...t,
        content,
        dirty: content !== t.original,
      }));
    },

    saving(source: EditorSource, path: string, saving: boolean) {
      mutate(source, path, (t) => ({ ...t, saving }));
    },

    saved(source: EditorSource, path: string) {
      mutate(source, path, (t) => ({
        ...t,
        saving: false,
        original: t.content,
        dirty: false,
      }));
    },

    error(source: EditorSource, path: string, error: string) {
      mutate(source, path, (t) => ({
        ...t,
        loading: false,
        saving: false,
        error,
      }));
    },
  };
}

export const fileEditors = createFileEditors();
