<script lang="ts">
  // Add/edit snippet form (tech-gui.md §2.2). Validation mirrors the TUI via
  // `formToSnippet`; on submit the parent persists + refreshes, and a rejected save
  // surfaces inline without closing. Semantic tokens only.
  import { onMount } from 'svelte';
  import type { SnippetDto } from '$lib/bindings';
  import { Button } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import Select from '$lib/components/Select.svelte';
  import { formToSnippet, type SnippetFormFields } from './snippetForm';

  let {
    mode,
    initial,
    previousName,
    onSubmit,
    onCancel
  }: {
    mode: 'add' | 'edit';
    initial: SnippetFormFields;
    previousName?: string;
    onSubmit: (snippet: SnippetDto, previousName: string | undefined) => Promise<void>;
    onCancel: () => void;
  } = $props();

  // Seeded once from `initial`; the editor is remounted per open, so the prop never
  // changes under a live instance.
  // svelte-ignore state_referenced_locally
  let fields = $state<SnippetFormFields>({ ...initial });
  let error = $state<string | null>(null);
  let saving = $state(false);
  let nameEl = $state<HTMLInputElement>();

  onMount(() => nameEl?.focus());

  async function save(): Promise<void> {
    const result = formToSnippet(fields);
    if (!result.ok) {
      error = result.error;
      return;
    }
    error = null;
    saving = true;
    try {
      await onSubmit(result.snippet, previousName);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  const label = 'block space-y-1 text-xs font-medium text-muted';
  const field =
    'w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none ' +
    'focus-visible:ring-2 focus-visible:ring-focus placeholder:text-faint';
</script>

<Modal label={mode === 'add' ? 'New snippet' : 'Edit snippet'} onClose={onCancel}>
  <form
    onsubmit={(e) => {
      e.preventDefault();
      void save();
    }}
    class="flex min-h-0 flex-col"
  >
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="text-sm font-semibold">{mode === 'add' ? 'New snippet' : 'Edit snippet'}</h2>
    </header>

    <div class="min-h-0 flex-1 space-y-3.5 overflow-y-auto px-5 py-4">
      <label class={label}>
        <span>Name</span>
        <input bind:this={nameEl} bind:value={fields.name} class={field} placeholder="restart-service" />
      </label>

      <label class={label}>
        <span>Command</span>
        <textarea
          bind:value={fields.command}
          rows="3"
          class="{field} resize-y font-mono"
          placeholder="systemctl restart {'{{service}}'}"
        ></textarea>
      </label>

      <div class="grid grid-cols-2 gap-3">
        <label class={label}>
          <span>Scope</span>
          <Select bind:value={fields.scope} class={field}>
            <option value="global">global</option>
            <option value="host">host</option>
          </Select>
        </label>
        <label class={label}>
          <span>Host {fields.scope === 'host' ? '(required)' : '(optional)'}</span>
          <input bind:value={fields.host} class={field} placeholder="web-1" />
        </label>
      </div>

      <label class={label}>
        <span>Tags</span>
        <input bind:value={fields.tags} class={field} placeholder="ops, deploy" />
      </label>

      <div class="space-y-1">
        <label class={label}>
          <span>Params</span>
          <input bind:value={fields.params} class={field} placeholder="service, timeout" />
        </label>
        <p class="text-[11px] text-faint">
          Comma-separated names, referenced as {'{{name}}'} in the command.
        </p>
      </div>

      {#if error}
        <p class="text-xs text-status-crit">{error}</p>
      {/if}
    </div>

    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <Button variant="ghost" onclick={onCancel}>Cancel</Button>
      <Button variant="primary" type="submit" disabled={saving}>
        {mode === 'add' ? 'Add snippet' : 'Save'}
      </Button>
    </footer>
  </form>
</Modal>
