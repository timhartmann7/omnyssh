<script lang="ts">
  // A live terminal tab (tech-gui.md §3.1). One instance per terminal session, kept
  // mounted for the session's whole life — hidden, not destroyed, when another entity
  // is active — so scrollback and the byte stream survive tab switches. Raw output
  // arrives on a per-session channel (§3.3/§3.6); keystrokes/resizes go back over the
  // terminal commands. Subscribes to the theme store and re-themes live (§5.1).
  import '@xterm/xterm/css/xterm.css';
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import type { Terminal } from '@xterm/xterm';
  import type { FitAddon } from '@xterm/addon-fit';
  import { Channel } from '@tauri-apps/api/core';
  import { theme } from '$lib/stores/theme';
  import { xtermTheme } from '$lib/theme/terminalTheme';
  import { sessions, type Session } from '$lib/stores/sessions';
  import { closeSession } from '$lib/stores/navigation';
  import { terminalDidExit } from '$lib/ipc/router';
  import { lastError } from '$lib/stores/notifications';
  import { dialogs } from '$lib/stores/dialogs';
  import { palette } from '$lib/stores/palette';
  import { isStreamerChord } from '$lib/stores/ui';
  import { activeEntity } from '$lib/stores/activeEntity';
  import {
    terminalOpen,
    terminalWrite,
    terminalResize,
    terminalClose,
    terminalPaste,
    clipboardReadText,
    clipboardWriteText
  } from '$lib/ipc/commands';
  import { shouldFadeTop } from './terminalFade';
  import {
    bufferText,
    chunkBytes,
    closesEndedTab,
    isCopyAllShortcut,
    isCopyInsert,
    isCopyShortcut,
    isMultiLine,
    isPasteShortcut,
    layoutFallback,
    lineCount,
    middleClickPastes,
    rightClickAction,
    trimCopied
  } from './terminalInput';
  import { clipboardPrefs } from '$lib/stores/clipboard';
  import Modal from '$lib/components/Modal.svelte';
  import { isMac, isWindows } from '$lib/platform';
  import type { TerminalBytes } from '$lib/bindings';

  let { session, active }: { session: Session; active: boolean } = $props();

  // The Nerd Font families come after the generic `monospace`, not merely after the
  // named system ones: the named list is macOS/Windows-only, so on a Linux desktop a
  // patched font ahead of the generic would become the terminal's Latin face and size
  // its cell from itself. Per-character fallback continues past a generic family, so the
  // Private Use Area glyphs (starship, powerlevel10k, eza --icons) still reach the tail.
  // Within the tail, the single-width variants come first — Nerd Fonts v3 ships icons at
  // double width in the bare family and one cell wide in its `Mono` twin.
  const MONO =
    'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace, ' +
    '"Symbols Nerd Font Mono", "Symbols Nerd Font", "MesloLGS NF", ' +
    '"JetBrainsMono Nerd Font Mono", "JetBrainsMono Nerd Font", ' +
    '"Hack Nerd Font Mono", "Hack Nerd Font", ' +
    '"FiraCode Nerd Font Mono", "FiraCode Nerd Font"';
  // One encoder for the keystroke hot path instead of one per input event.
  const ENCODER = new TextEncoder();

  // A large paste arrives as one onData; sending it as a single number[] would freeze
  // the UI thread (§9). Split into bounded chunks and await each so paint yields between
  // them; a serialization chain keeps all input strictly in order across events.
  let writeChain: Promise<void> = Promise.resolve();
  function sendInput(bytes: Uint8Array): void {
    if (termId == null || bytes.length === 0 || ended()) return;
    writeChain = writeChain.then(async () => {
      for (const chunk of chunkBytes(bytes)) {
        if (destroyed || termId == null) return;
        try {
          await terminalWrite(termId, Array.from(chunk));
        } catch {
          // Stop this input on a write failure rather than sending a gapped stream.
          return;
        }
      }
    });
  }

  /** No dialog or palette holds the keyboard: keystrokes meant for a key passphrase
   *  must never reach the shell. */
  function keyboardFree(): boolean {
    return get(dialogs).length === 0 && !get(palette).open;
  }

  /** The remote side ended the session: the tab stays for its last output, and the
   *  backend has nothing left to write or resize. Read from the store, which a late
   *  channel message can follow the exit into. */
  function ended(): boolean {
    return get(sessions).some((s) => s.id === session.id && s.status === 'closed');
  }

  let container: HTMLDivElement;
  let term: Terminal | undefined;
  let fitAddon: FitAddon | undefined;
  let termId: number | undefined;
  let destroyed = false;
  let connected = false;
  let ready = $state(false);
  let themeUnsub: (() => void) | undefined;
  let resizeObserver: ResizeObserver | undefined;
  let fitScheduled = false;
  // The top-edge fade dissolves scrolled output into the top edge, but never the live
  // prompt: after `clear`/Ctrl+L the cursor homes to the top, so the fade must lift
  // there (see terminalFade). Recomputed after every write too, since those resets
  // move the viewport without firing onScroll.
  let scrolled = $state(false);
  function syncScrolled(): void {
    const buf = term?.buffer.active;
    scrolled = !!buf && shouldFadeTop(buf.viewportY, buf.baseY, buf.cursorY);
  }

  // ── Copy & paste (PuTTY-style; see stores/clipboard) ──────────────────────────────
  // Both directions go through the native clipboard (commands/clipboard.rs): copy on
  // select happens outside any key gesture, which `navigator.clipboard` refuses there.
  const errText = (err: unknown): string => (err instanceof Error ? err.message : String(err));

  /** Copy the current selection, if any. Returns whether there was one. */
  function copySelection(): boolean {
    if (!term?.hasSelection()) return false;
    let text = term.getSelection();
    if (clipboardPrefs.get().trimTrailingWhitespace) text = trimCopied(text);
    if (!text) return false;
    clipboardWriteText(text).catch((err) => lastError.set(`Copy failed: ${errText(err)}`));
    return true;
  }

  /** A short note over the terminal ("Copied 312 lines"), so a copy with nothing to
   *  see on screen still shows it happened. */
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function flash(message: string): void {
    toast = message;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), 1600);
  }

  /** Copy the whole buffer — scrollback and screen — like PuTTY's "Copy All to
   *  Clipboard". Line endings follow the platform, as xterm's own selection does. */
  function copyAll(): void {
    if (!term) return;
    const text = bufferText(term.buffer.active, isWindows ? '\r\n' : '\n');
    if (!text) {
      flash('Nothing to copy');
      return;
    }
    const lines = text.split(isWindows ? '\r\n' : '\n').length;
    clipboardWriteText(text).then(
      () => flash(`Copied ${lines} line${lines === 1 ? '' : 's'}`),
      (err) => lastError.set(`Copy failed: ${errText(err)}`)
    );
  }

  // ── The terminal menu: Ctrl+right-click (PuTTY), or every right-click when the
  //    setting says so.
  let menu = $state<{ x: number; y: number; hasSelection: boolean } | null>(null);

  function openMenu(e: MouseEvent): void {
    // Kept inside the window: the menu is ~11rem wide and ~10rem tall.
    const x = Math.min(e.clientX, window.innerWidth - 184);
    const y = Math.min(e.clientY, window.innerHeight - 168);
    menu = { x: Math.max(4, x), y: Math.max(4, y), hasSelection: !!term?.hasSelection() };
  }

  function closeMenu(): void {
    if (!menu) return;
    menu = null;
    if (get(dialogs).length === 0) term?.focus();
  }

  function menuAction(action: 'copy' | 'paste' | 'copyAll' | 'selectAll'): void {
    closeMenu();
    if (action === 'copy') copySelection();
    else if (action === 'paste') pasteFromClipboard();
    else if (action === 'copyAll') copyAll();
    else term?.selectAll();
  }

  // While the menu is open, Escape closes it and any press elsewhere dismisses it.
  function onMenuKey(e: KeyboardEvent): void {
    if (menu && e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeMenu();
    }
  }
  function onMenuOutside(e: MouseEvent): void {
    if (menu && !(e.target as Element | null)?.closest?.('[data-term-menu]')) menu = null;
  }

  /** A multi-line paste waiting on the user's say-so, when that check is on. */
  let pendingPaste = $state<string | null>(null);

  /** Hand `text` to xterm, which applies bracketed paste when the remote asked for it
   *  and turns line breaks into the CR a terminal sends for Enter. */
  function deliverPaste(text: string): void {
    if (!term || !text || ended()) return;
    term.paste(text);
  }

  function pasteText(text: string): void {
    if (!text || ended()) return;
    if (clipboardPrefs.get().confirmMultiLinePaste && isMultiLine(text)) {
      pendingPaste = text;
      return;
    }
    deliverPaste(text);
  }

  /** Paste the system clipboard. If the native read fails, fall back to the webview's
   *  own paste (WebKitGTK), which lands in xterm's textarea like a Ctrl+Shift+V. */
  function pasteFromClipboard(): void {
    if (ended()) return;
    clipboardReadText().then(pasteText, () =>
      terminalPaste().catch((err) => lastError.set(`Paste failed: ${errText(err)}`))
    );
  }

  function confirmPaste(): void {
    const text = pendingPaste;
    pendingPaste = null;
    if (text) deliverPaste(text);
  }

  /** The remote app is tracking the mouse (vim `mouse=a`, tmux, htop). */
  function mouseTracking(): boolean {
    return (term?.modes.mouseTrackingMode ?? 'none') !== 'none';
  }

  // Set once copy-on-select has copied the current selection, so the second release of
  // a double-click (or a click that leaves the selection be) does not copy it again.
  // Any change to the selection clears it.
  let autoCopied = false;

  // Mouse hooks on the host element. Registered once xterm has mounted into it.
  function onMouseUp(e: MouseEvent): void {
    // Copy on select: after a drag, a double-click word or a triple-click line.
    // Deferred a tick so xterm has settled the selection from this same mouseup.
    if (e.button !== 0 || destroyed || !clipboardPrefs.get().copyOnSelect) return;
    setTimeout(() => {
      if (!autoCopied && copySelection()) autoCopied = true;
    }, 0);
  }

  function onContextMenu(e: MouseEvent): void {
    // The webview's own menu (Reload / Inspect) never belongs over a terminal.
    e.preventDefault();
    const action = rightClickAction(
      clipboardPrefs.get().rightClick,
      !!term?.hasSelection(),
      mouseTracking(),
      e.shiftKey,
      e.ctrlKey
    );
    if (action === 'menu') {
      openMenu(e);
      return;
    }
    if (action === 'copy') {
      copySelection();
      term?.clearSelection();
    } else if (action === 'paste') {
      pasteFromClipboard();
    }
    term?.focus();
  }

  // Set while a middle-click is ours, so its release is swallowed too: Linux webviews
  // paste the PRIMARY selection on the release, which would paste a second time.
  let middleTaken = false;

  function onMiddleUp(e: MouseEvent): void {
    if (e.button !== 1 || !middleTaken) return;
    middleTaken = false;
    e.preventDefault();
    e.stopPropagation();
  }

  function onMouseDown(e: MouseEvent): void {
    if (e.button === 0) {
      // A drag can end outside the terminal (xterm keeps selecting past its edge), so
      // the release is watched on the window, once per press.
      window.addEventListener('mouseup', onMouseUp, { once: true });
      return;
    }
    if (e.button !== 1) return;
    if (!middleClickPastes(clipboardPrefs.get().middleClickPaste, mouseTracking(), e.shiftKey)) return;
    // Stops Windows' autoscroll puck and WebKitGTK's own PRIMARY paste (which would
    // paste a second time), and keeps the click from xterm.
    e.preventDefault();
    e.stopPropagation();
    middleTaken = true;
    pasteFromClipboard();
    term?.focus();
  }

  /** A paste the webview delivers itself (macOS Cmd+V from the Edit menu, or the
   *  fallback above) still gets the multi-line check. Capture phase, so this runs
   *  before xterm's own textarea handler. */
  function onNativePaste(e: ClipboardEvent): void {
    if (!clipboardPrefs.get().confirmMultiLinePaste) return;
    const text = e.clipboardData?.getData('text/plain') ?? '';
    if (!isMultiLine(text)) return;
    e.preventDefault();
    e.stopPropagation();
    pasteText(text);
  }

  /** Fit the terminal to its container and tell the backend, but only while visible —
   *  a hidden (display:none) container measures 0, so it refits when shown instead. */
  function safeFit(): void {
    if (!term || !fitAddon || !active) return;
    try {
      fitAddon.fit();
    } catch {
      return;
    }
    if (termId != null && !ended()) void terminalResize(termId, term.cols, term.rows).catch(() => {});
  }

  function scheduleFit(): void {
    if (fitScheduled) return;
    fitScheduled = true;
    requestAnimationFrame(() => {
      fitScheduled = false;
      safeFit();
    });
  }

  onMount(() => {
    void (async () => {
      const [{ Terminal }, { FitAddon }] = await Promise.all([
        import('@xterm/xterm'),
        import('@xterm/addon-fit')
      ]);
      if (destroyed) return;

      term = new Terminal({
        fontFamily: MONO,
        fontSize: 13,
        cursorBlink: true,
        scrollback: 5000,
        // Right-click is ours (paste / copy-or-paste), so it must not first replace
        // the selection with the word under the pointer, as xterm does on macOS.
        rightClickSelectsWord: false
      });
      fitAddon = new FitAddon();
      term.loadAddon(fitAddon);
      term.open(container);
      term.onScroll(syncScrolled);
      term.onSelectionChange(() => (autoCopied = false));
      container.addEventListener('contextmenu', onContextMenu);
      container.addEventListener('mousedown', onMouseDown, true);
      container.addEventListener('mouseup', onMiddleUp, true);
      container.addEventListener('auxclick', onMiddleUp, true);
      container.addEventListener('paste', onNativePaste, true);
      window.addEventListener('keydown', onMenuKey, true);
      window.addEventListener('mousedown', onMenuOutside, true);

      // The #1 theme-regression guard (§5.1): push the matching xterm theme to this
      // terminal — including already-open ones — whenever the store flips. Its
      // synchronous first call (pre-paint) also sets the initial theme.
      themeUnsub = theme.subscribe((t) => {
        if (term) term.options.theme = xtermTheme(t);
      });

      // Route raw output into xterm. The channel is typed `number[]`, but the raw path
      // actually delivers an `ArrayBuffer` (§3.3); `Uint8Array` wraps either.
      const channel = new Channel<TerminalBytes>();
      channel.onmessage = (msg) => {
        if (!term) return;
        if (!connected) {
          connected = true;
          // A large first chunk travels asynchronously and can land after the exit.
          if (!ended()) sessions.setStatus(session.id, 'connected');
        }
        term.write(new Uint8Array(msg as unknown as ArrayBuffer), syncScrolled);
      };

      // Fit before opening so the remote PTY starts at the visible size.
      safeFit();
      const id = await terminalOpen(session.hostName, term.cols || 80, term.rows || 24, channel);
      if (destroyed) {
        void terminalClose(id).catch(() => {});
        return;
      }
      termId = id;
      sessions.setTermId(session.id, id);
      // The remote may have already exited before this id was recorded (fast-fail
      // connect race): terminal-exited couldn't match the tab, so settle it now.
      const exited = terminalDidExit(id);
      if (exited === false) {
        closeSession(session.id);
        return;
      }
      if (exited) sessions.setStatus(session.id, 'closed');

      // Copy takes Ctrl+Shift+C whether or not anything is selected, so the chord never
      // reaches the shell. Returning false only keeps xterm out of it; the default is
      // ours to stop. The write happens inside the keydown, which WebKit requires.
      term.attachCustomKeyEventHandler((e) => {
        // The streamer chord belongs to the window (AppShell), never to the shell, where
        // a ^S would stop its output.
        if (isStreamerChord(e)) return false;
        if (isCopyAllShortcut(e, isMac)) {
          e.preventDefault();
          copyAll();
          return false;
        }
        if (isCopyShortcut(e, isMac) || isCopyInsert(e)) {
          e.preventDefault();
          copySelection();
          return false;
        }
        // Shift+Insert, and Ctrl+Shift+V off macOS: paste from the native clipboard
        // instead of the webview's, so every paste takes the same path.
        if (isPasteShortcut(e, isMac)) {
          e.preventDefault();
          e.stopPropagation();
          pasteFromClipboard();
          return false;
        }
        // An ended session takes no input: Enter or Esc closes the tab, and xterm gets
        // no other key (`sendInput` drops anything that still gets through).
        if (ended()) {
          if (closesEndedTab(e)) {
            e.preventDefault();
            closeSession(session.id);
          } else if (e.type === 'keydown' && e.key === 'Tab') {
            // xterm no longer takes Tab here, and moving focus off the terminal
            // would hand the Enter that closes the tab to a button.
            e.preventDefault();
          }
          return false;
        }
        // Under a non-Latin layout WebKitGTK names no key; the physical one stands in.
        const fallback = layoutFallback(e);
        if (!fallback) return true;
        // As xterm does with a key it handles: nothing else acts on it.
        e.preventDefault();
        e.stopPropagation();
        if (fallback.kind === 'control') {
          term?.input(fallback.data);
        } else {
          pasteFromClipboard();
        }
        return false;
      });
      // Text keystrokes/paste are UTF-8; onBinary carries raw 8-bit sequences
      // (e.g. legacy mouse reporting) that must go byte-for-byte, not re-encoded.
      term.onData((data) => sendInput(ENCODER.encode(data)));
      term.onBinary((data) => sendInput(Uint8Array.from(data, (ch) => ch.charCodeAt(0) & 0xff)));

      resizeObserver = new ResizeObserver(() => scheduleFit());
      resizeObserver.observe(container);

      ready = true;
      if (active && keyboardFree()) term.focus();
    })().catch((err) => {
      // `terminal_open` itself failed (e.g. the session could not be spawned): no
      // PtyExited follows, so mark the tab failed here instead of leaving it hung.
      lastError.set(err instanceof Error ? err.message : String(err));
      sessions.setStatus(session.id, 'failed');
    });
  });

  onDestroy(() => {
    destroyed = true;
    themeUnsub?.();
    resizeObserver?.disconnect();
    window.removeEventListener('mouseup', onMouseUp);
    container?.removeEventListener('contextmenu', onContextMenu);
    container?.removeEventListener('mousedown', onMouseDown, true);
    container?.removeEventListener('mouseup', onMiddleUp, true);
    container?.removeEventListener('auxclick', onMiddleUp, true);
    container?.removeEventListener('paste', onNativePaste, true);
    window.removeEventListener('keydown', onMenuKey, true);
    window.removeEventListener('mousedown', onMenuOutside, true);
    clearTimeout(toastTimer);
    // Idempotent: a remote-exit teardown already dropped this id backend-side (§3.4).
    if (termId != null) void terminalClose(termId).catch(() => {});
    term?.dispose();
    // Null it so a byte still in flight (destroyed-before-open race) can't write to
    // a disposed terminal — the channel callback's `if (!term)` guard then bails.
    term = undefined;
  });

  // Becoming visible: a hidden container measured 0, so refit and take focus. Also
  // when a dialog or the palette closes, and when the active tab is clicked again,
  // which moves focus to the tab without changing `active`. The keyboard check runs
  // in the frame, since a dialog can open before it.
  $effect(() => {
    if (!active) menu = null;
  });

  $effect(() => {
    if (active && ready) {
      void [$dialogs, $palette, $activeEntity];
      requestAnimationFrame(() => {
        safeFit();
        if (keyboardFree()) term?.focus();
        syncScrolled();
      });
    }
  });
</script>

<!-- bg-surface fills behind the macOS traffic lights (no seam). Text selection stays
     disabled app-wide (app.css); the terminal is the one selectable surface, handled
     by xterm's own selection (not CSS). -->
<div class="absolute inset-0 overflow-hidden bg-surface {active ? '' : 'hidden'}">
  <!-- Inset via this wrapper, not the xterm host: padding on the element xterm mounts
       into makes FitAddon over-size, sliding the last row under the status bar. The top
       inset clears the macOS traffic-light strip; the bottom gap clears the footer. -->
  <div class="h-full w-full" style="padding: max(var(--titlebar-h), 0.75rem) 0.5rem 1rem;">
    <div bind:this={container} class="h-full w-full" class:term-fade={scrolled}></div>
  </div>

  {#if toast}
    <div
      role="status"
      class="pointer-events-none absolute bottom-6 right-6 rounded-full bg-surface-raised px-3 py-1.5 text-xs text-fg shadow-soft"
    >
      {toast}
    </div>
  {/if}
</div>

{#if menu}
  <div
    data-term-menu
    role="menu"
    aria-label="Terminal"
    class="fixed z-40 w-44 overflow-hidden rounded-xl border border-default bg-surface-raised py-1 text-sm shadow-soft"
    style="left: {menu.x}px; top: {menu.y}px;"
  >
    {@render menuItem('Copy', isMac ? '⌘C' : 'Ctrl+Shift+C', menu.hasSelection, () => menuAction('copy'))}
    {@render menuItem('Paste', isMac ? '⌘V' : 'Ctrl+Shift+V', true, () => menuAction('paste'))}
    <div class="my-1 border-t border-default"></div>
    {@render menuItem('Copy all', isMac ? '⇧⌘A' : 'Ctrl+Shift+A', true, () => menuAction('copyAll'))}
    {@render menuItem('Select all', '', true, () => menuAction('selectAll'))}
  </div>
{/if}

{#snippet menuItem(label: string, keys: string, enabled: boolean, run: () => void)}
  <button
    type="button"
    role="menuitem"
    disabled={!enabled}
    onclick={run}
    class="flex w-full items-center justify-between gap-3 px-3 py-1.5 text-left transition hover:bg-accent hover:text-accent-fg disabled:pointer-events-none disabled:opacity-40"
  >
    <span>{label}</span>
    {#if keys}<span class="text-xs opacity-60">{keys}</span>{/if}
  </button>
{/snippet}

{#if pendingPaste !== null}
  <Modal label="Confirm paste" onClose={() => (pendingPaste = null)}>
    <div class="p-5">
      <h2 class="text-sm font-semibold">Paste {lineCount(pendingPaste)} line{lineCount(pendingPaste) === 1 ? '' : 's'}?</h2>
      <p class="mt-1 text-xs text-muted">
        This text contains a line break, so the shell may run it as soon as it lands.
      </p>
      <pre
        class="mt-3 max-h-48 overflow-auto rounded-lg bg-surface-inset p-3 font-mono text-xs leading-relaxed">{pendingPaste.length > 4000
          ? pendingPaste.slice(0, 4000) + '\n…'
          : pendingPaste}</pre>
      <div class="mt-4 flex justify-end gap-2">
        <button
          type="button"
          class="rounded-full px-4 py-1.5 text-sm text-muted transition hover:bg-surface-inset hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
          onclick={() => (pendingPaste = null)}
        >
          Cancel
        </button>
        <!-- svelte-ignore a11y_autofocus -->
        <button
          type="button"
          autofocus
          class="rounded-full bg-accent px-4 py-1.5 text-sm text-accent-fg transition hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
          onclick={confirmPaste}
        >
          Paste
        </button>
      </div>
    </div>
  </Modal>
{/if}

<style>
  /* Scrolled output dissolves into the top edge instead of hard-clipping (on only while
     scrolled, so the first line stays crisp). black/transparent are mask alphas. */
  .term-fade {
    -webkit-mask-image: linear-gradient(to bottom, transparent, black 2.25rem);
    mask-image: linear-gradient(to bottom, transparent, black 2.25rem);
  }
</style>
