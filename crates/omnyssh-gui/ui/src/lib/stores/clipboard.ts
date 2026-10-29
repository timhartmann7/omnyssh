import { writable } from 'svelte/store';

// Terminal copy/paste behaviour, PuTTY-style by default: selecting text copies it,
// right-click and middle-click paste, Shift+Insert pastes and Ctrl+Insert copies. A UI
// pref persisted like the others (tauri-plugin-store + a localStorage mirror for first
// paint, tech-gui.md §4.3); the terminal reads it on every mouse/key event.

/** What a right-click in the terminal does.
 *  - `paste`: always paste (PuTTY).
 *  - `copyOrPaste`: copy and clear the selection if there is one, else paste
 *    (Windows Terminal).
 *  - `menu`: open the terminal's menu (Copy, Paste, Copy all, Select all).
 *  - `off`: do nothing (the webview's own menu stays suppressed).
 *  Ctrl+right-click opens that menu whatever this says, as in PuTTY. */
export type RightClick = 'paste' | 'copyOrPaste' | 'menu' | 'off';
export const RIGHT_CLICK_OPTIONS: readonly RightClick[] = ['paste', 'copyOrPaste', 'menu', 'off'];

export interface ClipboardPrefs {
  /** Copy the selection to the clipboard as soon as the mouse is released. */
  copyOnSelect: boolean;
  rightClick: RightClick;
  /** Middle-click pastes (the X11 / PuTTY habit). */
  middleClickPaste: boolean;
  /** Strip trailing spaces from each copied line, as PuTTY does. */
  trimTrailingWhitespace: boolean;
  /** Ask before a paste that contains a line break runs it straight into the shell. */
  confirmMultiLinePaste: boolean;
}

export const CLIPBOARD_DEFAULTS: ClipboardPrefs = {
  copyOnSelect: true,
  rightClick: 'paste',
  middleClickPaste: true,
  trimTrailingWhitespace: true,
  confirmMultiLinePaste: false
};

const LOCAL_KEY = 'omnyssh-clipboard';
const STORE_FILE = 'settings.json';
const STORE_KEY = 'clipboard';

/** Read a stored value leniently: a missing or malformed field keeps its default. */
export function readPrefs(value: unknown): ClipboardPrefs {
  const v = (value && typeof value === 'object' ? value : {}) as Record<string, unknown>;
  const bool = (key: keyof ClipboardPrefs): boolean =>
    typeof v[key] === 'boolean' ? (v[key] as boolean) : (CLIPBOARD_DEFAULTS[key] as boolean);
  const rightClick = RIGHT_CLICK_OPTIONS.includes(v.rightClick as RightClick)
    ? (v.rightClick as RightClick)
    : CLIPBOARD_DEFAULTS.rightClick;
  return {
    copyOnSelect: bool('copyOnSelect'),
    rightClick,
    middleClickPaste: bool('middleClickPaste'),
    trimTrailingWhitespace: bool('trimTrailingWhitespace'),
    confirmMultiLinePaste: bool('confirmMultiLinePaste')
  };
}

function mirrored(): ClipboardPrefs {
  try {
    const raw = localStorage.getItem(LOCAL_KEY);
    return raw == null ? { ...CLIPBOARD_DEFAULTS } : readPrefs(JSON.parse(raw));
  } catch {
    return { ...CLIPBOARD_DEFAULTS };
  }
}

function mirrorLocal(prefs: ClipboardPrefs): void {
  try {
    localStorage.setItem(LOCAL_KEY, JSON.stringify(prefs));
  } catch {
    // localStorage unavailable (hardened webview): the store copy is canonical.
  }
}

async function persistStore(prefs: ClipboardPrefs): Promise<void> {
  try {
    const { load } = await import('@tauri-apps/plugin-store');
    const store = await load(STORE_FILE);
    await store.set(STORE_KEY, prefs);
    await store.save();
  } catch {
    // Not under Tauri (tests, vite preview): the localStorage mirror suffices.
  }
}

function createClipboardPrefs() {
  const initial = mirrored();
  const { subscribe, set: setStore } = writable<ClipboardPrefs>(initial);
  let current = initial;
  let interacted = false;

  function apply(prefs: ClipboardPrefs, user: boolean): void {
    current = prefs;
    setStore(prefs);
    mirrorLocal(prefs);
    if (user) {
      interacted = true;
      void persistStore(prefs);
    }
  }

  return {
    subscribe,
    /** The current value, for event handlers that should not subscribe. */
    get: (): ClipboardPrefs => current,
    /** Change one or more settings, as the user did. */
    update: (change: Partial<ClipboardPrefs>) => apply(readPrefs({ ...current, ...change }), true),
    /** Put every setting back to the PuTTY-style defaults. */
    reset: () => apply({ ...CLIPBOARD_DEFAULTS }, true),
    /** Reconcile with the canonical tauri-plugin-store value once Tauri is reachable. */
    async hydrate(): Promise<void> {
      try {
        const { load } = await import('@tauri-apps/plugin-store');
        const store = await load(STORE_FILE);
        const saved = await store.get<unknown>(STORE_KEY);
        if (!interacted && saved !== undefined) apply(readPrefs(saved), false);
      } catch {
        // Store unreachable: keep the mirrored value.
      }
    }
  };
}

export const clipboardPrefs = createClipboardPrefs();
