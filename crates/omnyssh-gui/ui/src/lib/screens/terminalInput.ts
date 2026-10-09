// Terminal input is sent over `terminal_write` as a `number[]` (§4.2). A single huge
// paste would serialize as one giant array on the main thread and freeze the UI, so the
// view splits it into bounded chunks and awaits each (yielding between). This is the
// pure split; the view owns the ordered dispatch. The Ctrl chords xterm cannot handle
// itself — the copy shortcut, and those WebKitGTK leaves unnamed under a non-Latin
// layout — are decided here too, before xterm turns the key into input.

/** The per-write byte cap. Small enough that one chunk's `number[]` serialization is
 *  imperceptible, so a multi-MB paste streams without a visible stall (§9). */
export const INPUT_CHUNK = 8192;

/** The fields of a key event these chords read, so tests can pass plain objects. */
export type KeyPress = Pick<
  KeyboardEvent,
  | 'type'
  | 'key'
  | 'code'
  | 'keyCode'
  | 'ctrlKey'
  | 'shiftKey'
  | 'altKey'
  | 'metaKey'
  | 'isComposing'
>;

/** A letter of a non-Latin script (Cyrillic, Greek, Hebrew…): the one case where the
 *  physical key, not the letter, names a Ctrl chord. A Latin letter with a diacritic
 *  (ü, å, ç) or a dead key keeps its own meaning. */
function nonLatinLetter(key: string): boolean {
  return /^\p{L}$/u.test(key) && !/\p{Script=Latin}/u.test(key);
}

/** Ctrl+Shift+C on Windows and Linux, as in GNOME Terminal and Windows Terminal: a bare
 *  Ctrl+C has to stay ^C. macOS needs none — Cmd+C copies there through the Edit menu.
 *  The webview pastes Ctrl+Shift+V natively, except where `layoutFallback` steps in. */
export function isCopyShortcut(e: KeyPress, mac: boolean): boolean {
  // keyCode 229 marks the keydown that starts an IME composition.
  if (mac || e.type !== 'keydown' || e.isComposing || e.keyCode === 229) return false;
  if (e.altKey || e.metaKey || !e.ctrlKey || !e.shiftKey) return false;
  // The physical key stands in only where the layout puts a non-Latin letter on it, so
  // a Cyrillic layout copies with the same keys while Dvorak's J there stays a J.
  return e.key === 'c' || e.key === 'C' || (e.code === 'KeyC' && nonLatinLetter(e.key));
}

/** ⌘F on macOS; Ctrl+Shift+F on Windows and Linux, as in GNOME Terminal and Windows
 *  Terminal, because a bare Ctrl+F has to stay ^F (readline's forward-char, less's
 *  page down). Opens the terminal's find bar. */
export function isFindShortcut(e: KeyPress, mac: boolean): boolean {
  if (e.type !== 'keydown' || e.isComposing || e.keyCode === 229 || e.altKey) return false;
  if (mac ? !e.metaKey || e.ctrlKey || e.shiftKey : e.metaKey || !e.ctrlKey || !e.shiftKey) {
    return false;
  }
  return e.key === 'f' || e.key === 'F' || (e.code === 'KeyF' && nonLatinLetter(e.key));
}

/** Enter or Esc, unmodified and outside an IME composition: what closes a tab whose
 *  session has ended. */
export function closesEndedTab(e: KeyPress): boolean {
  if (e.type !== 'keydown' || e.isComposing || e.keyCode === 229) return false;
  if (e.ctrlKey || e.shiftKey || e.altKey || e.metaKey) return false;
  return e.key === 'Enter' || e.key === 'Escape';
}

/** What a Ctrl chord means when WebKitGTK cannot say. Under a non-Latin layout it
 *  reports keyCode 0 for the letters, so xterm sends nothing for Ctrl+C and the
 *  webview's own Ctrl+Shift+V never fires; the physical key decides instead, as in
 *  GNOME Terminal. Every other webview reports a Latin keyCode and needs none of this. */
export type LayoutFallback = { kind: 'control'; data: string } | { kind: 'paste' } | null;

// The punctuation Ctrl chords xterm maps by keyCode: ESC (Ctrl+[, vi's escape), FS, GS.
// A non-Latin layout puts letters on these keys too (х, ъ on a Russian one).
const CONTROL_PUNCTUATION: Record<string, string> = {
  BracketLeft: '\x1b',
  Backslash: '\x1c',
  BracketRight: '\x1d'
};

export function layoutFallback(e: KeyPress): LayoutFallback {
  if (e.type !== 'keydown' || e.keyCode !== 0 || e.isComposing) return null;
  if (!e.ctrlKey || e.altKey || e.metaKey || !nonLatinLetter(e.key)) return null;
  if (e.shiftKey) return e.code === 'KeyV' ? { kind: 'paste' } : null;
  const letter = /^Key([A-Z])$/.exec(e.code);
  if (letter) return { kind: 'control', data: String.fromCharCode(letter[1].charCodeAt(0) - 64) };
  const punctuation = CONTROL_PUNCTUATION[e.code];
  return punctuation ? { kind: 'control', data: punctuation } : null;
}

/** Split `data` into <=`size` slices, in order. Empty input yields nothing; input at or
 *  below the cap yields a single slice (the ordinary keystroke path). */
export function chunkBytes(data: Uint8Array, size: number = INPUT_CHUNK): Uint8Array[] {
  if (data.length <= size) return data.length ? [data] : [];
  const chunks: Uint8Array[] = [];
  for (let i = 0; i < data.length; i += size) {
    chunks.push(data.subarray(i, i + size));
  }
  return chunks;
}
