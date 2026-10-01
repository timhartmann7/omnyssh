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
  import {
    terminalOpen,
    terminalWrite,
    terminalResize,
    terminalClose,
    terminalPaste
  } from '$lib/ipc/commands';
  import { shouldFadeTop } from './terminalFade';
  import { chunkBytes, closesEndedTab, isCopyShortcut, layoutFallback } from './terminalInput';
  import { isMac } from '$lib/platform';
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
        scrollback: 5000
      });
      fitAddon = new FitAddon();
      term.loadAddon(fitAddon);
      term.open(container);
      term.onScroll(syncScrolled);

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
        if (isCopyShortcut(e, isMac)) {
          e.preventDefault();
          if (term?.hasSelection()) {
            navigator.clipboard.writeText(term.getSelection()).catch((err) => {
              lastError.set(`Copy failed: ${err instanceof Error ? err.message : String(err)}`);
            });
          }
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
          terminalPaste().catch((err) => {
            lastError.set(`Paste failed: ${err instanceof Error ? err.message : String(err)}`);
          });
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
      if (active && get(dialogs).length === 0) term.focus();
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
    // Idempotent: a remote-exit teardown already dropped this id backend-side (§3.4).
    if (termId != null) void terminalClose(termId).catch(() => {});
    term?.dispose();
    // Null it so a byte still in flight (destroyed-before-open race) can't write to
    // a disposed terminal — the channel callback's `if (!term)` guard then bails.
    term = undefined;
  });

  // Becoming visible: a hidden container measured 0, so refit and take focus.
  // An open dialog keeps the keyboard (keystrokes meant for a key passphrase must
  // never reach the shell); the terminal takes it back once the last one closes.
  $effect(() => {
    if (active && ready) {
      const free = $dialogs.length === 0;
      requestAnimationFrame(() => {
        safeFit();
        if (free) term?.focus();
        syncScrolled();
      });
    }
  });
</script>

<!-- bg-surface fills behind the macOS traffic lights. -->
<div class:hidden={!active} class="absolute inset-0 overflow-hidden bg-surface">
  <!-- Padding is applied here so FitAddon measures the correct terminal size. -->
  <div
    class="h-full w-full"
    style="padding: max(var(--titlebar-h), 0.75rem) 0.5rem 1rem;"
  >
    <div
      bind:this={container}
      class="h-full w-full"
      class:term-fade={scrolled}
    ></div>
  </div>
</div>

<style>
  .term-fade {
    -webkit-mask-image: linear-gradient(
      to bottom,
      transparent,
      black 2.25rem
    );
    mask-image: linear-gradient(
      to bottom,
      transparent,
      black 2.25rem
    );
  }
</style>
