<script lang="ts">
  // Minimal settings (tech-gui.md §4.3): the light/dark theme mirrored from the sidebar
  // (§5.1), the metric auto-refresh interval and the tray (tauri-plugin-store UI prefs),
  // and the update preferences (`check_on_startup`) + a manual update check. Update prefs
  // persist to the shared config via `save_update_config`; the rest are frontend prefs.
  import { onMount } from 'svelte';
  import type { UpdateConfigDto } from '$lib/bindings';
  import { Surface, Icon } from '$lib/theme';
  import { theme } from '$lib/stores/theme';
  import { streamerMode } from '$lib/stores/streamer';
  import { refreshInterval, REFRESH_OPTIONS } from '$lib/stores/settings';
  import { traySupport, trayBehavior } from '$lib/stores/tray';
  import { isMac } from '$lib/platform';
  import { offerUpdate } from '$lib/stores/update';
  import { lastError } from '$lib/stores/notifications';
import {
  checkUpdate,
  loadUpdateConfig,
  saveUpdateConfig,
  saveGeneralConfig
} from '$lib/ipc/commands';

import { generalConfigStore } from '$lib/stores/generalConfig';

  const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));
  const formatInterval = (secs: number): string => (secs < 60 ? `${secs}s` : `${secs / 60}m`);

  let updateConfig = $state<UpdateConfigDto | null>(null);
  // Raw external editor command used for large remote files.
  // Examples: zed --wait, code --wait, nvim, hx
  let editorSelection = $state('');
  let largeFileMb = $state(5);
  let promptLargeFiles = $state(true);
  let autoUploadExternal = $state(false);

  type CheckState =
    | { kind: 'idle' }
    | { kind: 'checking' }
    | { kind: 'upToDate' }
    | { kind: 'available'; version: string }
    | { kind: 'error'; message: string };
  let check = $state<CheckState>({ kind: 'idle' });

  onMount(async () => {
    try {
      updateConfig = await loadUpdateConfig();
      const config = await generalConfigStore.load();
      editorSelection = config.externalEditor ?? '';
      largeFileMb = config.largeFileMb;
      promptLargeFiles = config.promptLargeFiles;
      autoUploadExternal = config.autoUploadExternal;
    } catch (e) {
      lastError.set(message(e));
    }
  });

  // Persist immediately; revert the optimistic flip if the write fails. Read-modify-write
  // fresh rather than from the mount-time cache: the update banner may have written a
  // `skipVersion` out-of-band, and `save_update_config` replaces the whole [update]
  // section — writing a stale copy would clobber that skip.
  async function toggleCheckOnStartup(): Promise<void> {
    if (!updateConfig) return;
    const previous = updateConfig;
    const desired = !previous.checkOnStartup;
    updateConfig = { ...previous, checkOnStartup: desired };
    try {
      const current = await loadUpdateConfig();
      const next = { ...current, checkOnStartup: desired };
      await saveUpdateConfig(next);
      updateConfig = next;
    } catch (e) {
      updateConfig = previous;
      lastError.set(message(e));
    }
  }

// Save the GUI editor preferences immediately when the user leaves the field.
async function saveGeneral() {
  const current = generalConfigStore.value;
  if (!current) return;

  const next = {
    ...current,
    externalEditor: editorSelection.trim(),
    largeFileMb,
    promptLargeFiles,
    autoUploadExternal,
  };

  try {
    await saveGeneralConfig(next);
    generalConfigStore.set(next);
  } catch (e) {
    lastError.set(message(e));
  }
}

  async function checkNow(): Promise<void> {
    check = { kind: 'checking' };
    try {
      const info = await checkUpdate();
      if (info) {
        offerUpdate(info); // raise the banner too
        check = { kind: 'available', version: info.version };
      } else {
        check = { kind: 'upToDate' };
      }
    } catch (e) {
      check = { kind: 'error', message: message(e) };
    }
  }

  const seg =
    'rounded-lg px-3 py-1.5 text-sm transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const segState = (active: boolean): string =>
    active ? 'bg-accent text-accent-fg' : 'text-muted hover:bg-surface-inset hover:text-fg';
</script>

<section class="mx-auto h-full max-w-2xl p-6">
  <h1 class="mb-5 text-lg font-semibold tracking-tight">Settings</h1>

  <div class="space-y-4">
    <!-- Appearance -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">Appearance</h2>
      <div class="flex items-center justify-between gap-4">
        <div>
          <p class="text-sm">Theme</p>
          <p class="text-xs text-muted">Mirrors the sidebar toggle.</p>
        </div>
        <div class="flex gap-1 rounded-xl bg-surface-inset p-1">
          <button
            type="button"
            class="{seg} {segState($theme === 'light')}"
            aria-pressed={$theme === 'light'}
            onclick={() => theme.set('light')}
          >
            <span class="flex items-center gap-1.5"><Icon name="sun" size={14} /> Light</span>
          </button>
          <button
            type="button"
            class="{seg} {segState($theme === 'dark')}"
            aria-pressed={$theme === 'dark'}
            onclick={() => theme.set('dark')}
          >
            <span class="flex items-center gap-1.5"><Icon name="moon" size={14} /> Dark</span>
          </button>
        </div>
      </div>
    </Surface>

    <!-- Privacy -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">Privacy</h2>
      <div class="flex items-center justify-between gap-4">
        <div class="min-w-0">
          <p class="text-sm">Streamer mode</p>
          <p class="text-xs text-muted">
            Mask host addresses with realistic fakes, so real IPs stay off-screen while recording.
          </p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={$streamerMode}
          aria-label="Streamer mode"
          onclick={() => streamerMode.toggle()}
          class="relative h-6 w-11 shrink-0 rounded-full transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus {$streamerMode
            ? 'bg-accent'
            : 'bg-surface-inset'}"
        >
          <span
            class="absolute top-0.5 h-5 w-5 rounded-full bg-surface shadow-soft transition-[left] {$streamerMode
              ? 'left-[1.375rem]'
              : 'left-0.5'}"
          ></span>
        </button>
      </div>
    </Surface>

    <!-- Dashboard -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">Dashboard</h2>
      <div class="flex items-center justify-between gap-4">
        <div>
          <p class="text-sm">Auto-refresh interval</p>
          <p class="text-xs text-muted">How often the dashboard forces a metric refresh.</p>
        </div>
        <div class="flex flex-wrap justify-end gap-1 rounded-xl bg-surface-inset p-1">
          {#each REFRESH_OPTIONS as secs (secs)}
            <button
              type="button"
              class="{seg} tabular-nums {segState($refreshInterval === secs)}"
              aria-pressed={$refreshInterval === secs}
              onclick={() => refreshInterval.set(secs)}
            >
              {formatInterval(secs)}
            </button>
          {/each}
        </div>
      </div>
    </Surface>

    <!-- Window: the tray keeps sessions and tunnels up with no window on screen. macOS
         has no minimize event to act on, and keeps minimized windows in the Dock. -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">Window</h2>
      <div class="space-y-4">
        {#if !isMac}
          {@render traySwitch(
            'Minimize to tray',
            $traySupport.available && !$traySupport.minimize
              ? 'Not on Wayland, which never tells an app its window was minimized.'
              : 'Minimizing hides the window; the tray icon brings it back.',
            $trayBehavior.minimizeToTray && $traySupport.minimize,
            $traySupport.minimize,
            () => trayBehavior.update({ minimizeToTray: !$trayBehavior.minimizeToTray })
          )}
        {/if}

        {@render traySwitch(
          isMac ? 'Close to the menu bar' : 'Close to tray',
          'Closing the window keeps OmnySSH running — terminals, transfers and tunnels stay connected. Quit from the icon.',
          $trayBehavior.closeToTray && $traySupport.available,
          $traySupport.available,
          () => trayBehavior.update({ closeToTray: !$trayBehavior.closeToTray })
        )}

        {#if !$traySupport.available}
          <p class="text-xs text-status-warn">
            This desktop has no system tray, so the window closes and minimizes as usual.
          </p>
        {:else if $trayBehavior.minimizeToTray || $trayBehavior.closeToTray}
          <!-- Some desktops keep tray icons out of sight (GNOME without the AppIndicator
               extension, an overflow menu); a second launch always finds the window. -->
          <p class="text-xs text-muted">
            No icon in sight? Opening OmnySSH again brings the window back.
          </p>
        {/if}
      </div>
    </Surface>

    <!-- File Editing -->
    <Surface class="p-5">
      <h2 class="mb-4 text-sm font-semibold">File Editing</h2>

      {#if generalConfigStore.value}
        <div class="space-y-4">
          <div class="space-y-2">
            <label for="external-editor" class="text-sm font-medium">
              External editor
            </label>

            <input
              id="external-editor"
              bind:value={editorSelection}
              onblur={saveGeneral}
              onkeydown={(e) => e.key === 'Enter' && saveGeneral()}
              placeholder="zed --wait"
              class="w-full rounded-lg border border-default bg-surface px-3 py-2 font-mono text-sm"
            />

            <p class="text-xs text-muted">
              Command used for editing large remote files.
            </p>

            <p class="font-mono text-xs text-faint">
              Examples: zed --wait · code --wait · nvim · hx · fresh
            </p>
          </div>

          <div>
            <label for="large-file-threshold" class="mb-1 block text-sm">
              Large file threshold
            </label>

            <div class="flex items-center gap-2">
              <input
                id="large-file-threshold"
                type="number"
                min="1"
                max="1024"
                bind:value={largeFileMb}
                onchange={saveGeneral}
                class="w-28 rounded-lg border border-default bg-surface px-3 py-2 text-sm"
              />
              <span class="text-sm text-muted">MB</span>
            </div>

            <div class="mt-4 border-t border-default pt-4">
              <label class="flex items-center justify-between gap-4">
                <div>
                  <p class="text-sm">Auto-upload after external edit</p>
                  <p class="text-xs text-muted">
                    Skip the confirmation dialog and upload/save automatically.
                  </p>
                </div>

                <input
                  type="checkbox"
                  bind:checked={autoUploadExternal}
                  onchange={saveGeneral}
                  class="h-4 w-4"
                />
              </label>
            </div>
          </div>
        </div>
      {/if}
    </Surface>

    <!-- Updates -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">Updates</h2>
      <div class="space-y-4">
        <div class="flex items-center justify-between gap-4">
          <div>
            <p class="text-sm">Check for updates on startup</p>
            <p class="text-xs text-muted">Look for a newer release when the app launches.</p>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={updateConfig?.checkOnStartup ?? false}
            aria-label="Check for updates on startup"
            disabled={!updateConfig}
            onclick={toggleCheckOnStartup}
            class="relative h-6 w-11 shrink-0 rounded-full transition disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus {updateConfig?.checkOnStartup
              ? 'bg-accent'
              : 'bg-surface-inset'}"
          >
            <span
              class="absolute top-0.5 h-5 w-5 rounded-full bg-surface shadow-soft transition-[left] {updateConfig?.checkOnStartup
                ? 'left-[1.375rem]'
                : 'left-0.5'}"
            ></span>
          </button>
        </div>

        <div class="flex items-center justify-between gap-4 border-t border-default pt-4">
          <div class="min-w-0">
            <p class="text-sm">Manual check</p>
            <p class="text-xs text-muted">
              {#if check.kind === 'checking'}Checking…
              {:else if check.kind === 'upToDate'}You're on the latest version.
              {:else if check.kind === 'available'}Version {check.version} is available.
              {:else if check.kind === 'error'}{check.message}
              {:else}Check GitHub for a newer release.
              {/if}
            </p>
          </div>
          <button
            type="button"
            class="inline-flex shrink-0 items-center gap-1.5 rounded-full border border-strong px-4 py-1.5 text-sm text-fg transition hover:bg-accent hover:text-accent-fg disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
            disabled={check.kind === 'checking'}
            onclick={checkNow}
          >
            <Icon name="refresh" size={14} />
            Check now
          </button>
        </div>
      </div>
    </Surface>
  </div>
</section>

{#snippet traySwitch(
  label: string,
  hint: string,
  on: boolean,
  enabled: boolean,
  toggle: () => void
)}
  <div class="flex items-center justify-between gap-4">
    <div class="min-w-0">
      <p class="text-sm">{label}</p>
      <p class="text-xs text-muted">{hint}</p>
    </div>
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      disabled={!enabled}
      onclick={toggle}
      class="relative h-6 w-11 shrink-0 rounded-full transition disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus {on
        ? 'bg-accent'
        : 'bg-surface-inset'}"
    >
      <span
        class="absolute top-0.5 h-5 w-5 rounded-full bg-surface shadow-soft transition-[left] {on
          ? 'left-[1.375rem]'
          : 'left-0.5'}"
      ></span>
    </button>
  </div>
{/snippet}
