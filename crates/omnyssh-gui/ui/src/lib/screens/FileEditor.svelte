<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { commands, events } from '$lib/bindings';
  import { fileEditors, type EditorTab, type EditorSource } from '$lib/stores/fileEditor';

  let {
    sessionId,
    path,
    local = false
  }: {
    sessionId?: number;
    path: string;
    local?: boolean;
  } = $props();


  let tab = $state<EditorTab | undefined>(undefined);

let unsubscribeStore = () => {};

$effect(() => {
  const stop = fileEditors.subscribe((tabs) => {
    tab = tabs.find((t) => {
      if (local) {
        return t.source.kind === 'local' && t.path === path;
      }

      return (
        t.source.kind === 'remote' &&
        t.source.sessionId === sessionId &&
        t.path === path
      );
    });
  });

  if (local) {
    fileEditors.openLocal(path);
  } else if (sessionId != null) {
    fileEditors.open(sessionId, path);
  }

  unsubscribeStore = stop;

  return () => stop();
});

onMount(() => {
  void (async () => {

    const un1 = await events.fileContentReady.listen((e) => {
      if (
        !local &&
        e.payload.sessionId === sessionId &&
        e.payload.path === path
      ) {
        fileEditors.setContent(source, path, e.payload.content);
      }
    });

    const un2 = await events.fileContentReadFailed.listen((e) => {
      if (
        !local &&
        e.payload.sessionId === sessionId &&
        e.payload.path === path
      ) {
        fileEditors.error(source, path, e.payload.error);
      }
    });

    const un3 = await events.fileWriteDone.listen((e) => {
      if (
        !local ||
        e.payload.sessionId !== sessionId ||
        e.payload.path !== path
      ) return;

      if (e.payload.ok) {
        fileEditors.saved(source, path);
      } else {
        fileEditors.error(source, path, e.payload.error ?? "Save failed");
      }
    });

    unsubscribeEvents = () => {
      un1();
      un2();
      un3();
    };
  })();

  return () => {
    unsubscribeStore();
    unsubscribeEvents();
  };
});

  let unsubscribeEvents = () => {};

  const source = $derived(
  local
    ? ({ kind: 'local' } as const)
    : ({ kind: 'remote', sessionId: sessionId! } as const)
  );

  const lines = $derived(tab ? tab.content.split('\n').length : 1);

  function update(value: string) {
    fileEditors.edit(source, path, value);
  }

async function save() {
  if (!tab) return;

  fileEditors.saving(source, path, true);

  if (local) {
    const r = await commands.localWriteFile(path, tab.content);

    if (r.status === 'error') {
      fileEditors.error(source, path, r.error.message);
      return;
    }
  } else {
    const r = await commands.sftpWriteFile(
      sessionId!,
      path,
      tab.content
    );

    if (r.status === 'error') {
      fileEditors.error(source, path, r.error.message);
      return;
    }
  }

  fileEditors.saved(source, path);
}

  function keydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === 's') {
      e.preventDefault();
      save();
      return;
    }

    if (e.key !== 'Tab' || !tab) return;

    e.preventDefault();

    const el = e.target as HTMLTextAreaElement;
    const start = el.selectionStart;
    const end = el.selectionEnd;

    const value =
      tab.content.substring(0, start) +
      '    ' +
      tab.content.substring(end);

    update(value);

    requestAnimationFrame(() => {
      el.selectionStart = el.selectionEnd = start + 4;
    });
  }
</script>

{#if !tab}
  <div class="flex h-full items-center justify-center">
    Loading...
  </div>
{:else}
  <div class="flex h-full flex-col">
    <div class="flex items-center justify-between border-b px-3 py-2">
      <div class="flex items-center gap-2">
        <span class="font-medium">{tab.name}</span>
        {#if tab.dirty}
          <span class="text-xs text-amber-500">● Modified</span>
        {/if}
      </div>

      <button
        class="rounded border px-3 py-1 text-sm"
        onclick={save}
        disabled={!tab.dirty || tab.saving}
      >
        {#if tab.saving}Saving...{:else}Save{/if}
      </button>
    </div>

    {#if tab.error}
      <div class="border-b bg-red-50 px-3 py-2 text-sm text-red-700">
        {tab.error}
      </div>
    {/if}

    {#if tab.loading}
      <div class="flex flex-1 items-center justify-center">
        {#if local}
        Reading local file...
	{:else}
        Reading remote file...
	{/if}
      </div>
    {:else}
      <div class="flex flex-1 overflow-hidden font-mono text-sm">
        <div class="select-none border-r bg-muted/30 px-2 py-3 text-right text-muted-foreground">
          {#each Array(lines) as _, i}
            <div class="h-5">{i + 1}</div>
          {/each}
        </div>

<textarea
  class="flex-1 resize-none border-0 bg-transparent p-3 outline-none"
  spellcheck="false"
  value={tab.content}
  oninput={(e) => update((e.target as HTMLTextAreaElement).value)}
  onkeydown={keydown}
></textarea>
      </div>
    {/if}
  </div>
{/if}
