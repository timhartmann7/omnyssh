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

/** No modifiers but the ones named, outside an IME composition, on keydown. */
function plainChord(e: KeyPress, ctrl: boolean, shift: boolean): boolean {
  if (e.type !== 'keydown' || e.isComposing || e.keyCode === 229) return false;
  return e.ctrlKey === ctrl && e.shiftKey === shift && !e.altKey && !e.metaKey;
}

/** Ctrl+Insert: PuTTY's (and CUA's) other copy. Insert means nothing to a shell with
 *  Ctrl held, so it never needs to reach the remote. */
export function isCopyInsert(e: KeyPress): boolean {
  return plainChord(e, true, false) && e.key === 'Insert';
}

/** A paste chord the terminal answers itself, from the native clipboard: Shift+Insert
 *  everywhere (PuTTY, xterm), and Ctrl+Shift+V off macOS — taken from the webview so
 *  every paste goes through the same multi-line check. macOS keeps Cmd+V on its Edit
 *  menu, which the `paste` event hook covers. */
export function isPasteShortcut(e: KeyPress, mac: boolean): boolean {
  if (plainChord(e, false, true) && e.key === 'Insert') return true;
  if (mac || !plainChord(e, true, true)) return false;
  return e.key === 'v' || e.key === 'V' || (e.code === 'KeyV' && nonLatinLetter(e.key));
}

/** Strip the trailing spaces and tabs from every line of a copied selection, as PuTTY
 *  does, so a selection dragged past the end of a line carries no padding. Line
 *  endings (LF or CRLF) are kept as they are. */
export function trimCopied(text: string): string {
  return text.replace(/[ \t]+(?=\r?\n|$)/g, '');
}

/** Whether a paste would press Enter in the shell: any line break at all, trailing
 *  ones included, since `ls\n` runs as surely as two lines do. */
export function isMultiLine(text: string): boolean {
  return /[\r\n]/.test(text);
}

/** How many lines a paste holds, for the confirmation. A single trailing break does
 *  not start a line of its own. */
export function lineCount(text: string): number {
  const lines = text.split(/\r\n|\r|\n/);
  if (lines.length > 1 && lines[lines.length - 1] === '') lines.pop();
  return lines.length;
}

/** What a right-click does, given the setting and the terminal's state. Ctrl+right-
 *  click, or the `menu` setting, opens the terminal's menu instead. While the
 *  remote app tracks the mouse (vim `mouse=a`, tmux, htop) the click is the app's, as
 *  in PuTTY; Shift takes it back. */
export function rightClickAction(
  pref: 'paste' | 'copyOrPaste' | 'menu' | 'off',
  hasSelection: boolean,
  mouseTracking: boolean,
  shift: boolean,
  ctrl = false
): 'paste' | 'copy' | 'menu' | null {
  // Ctrl+right-click always opens the menu, as in PuTTY — even over a tracked mouse.
  if (ctrl || pref === 'menu') return 'menu';
  if (pref === 'off' || (mouseTracking && !shift)) return null;
  if (pref === 'copyOrPaste' && hasSelection) return 'copy';
  return 'paste';
}

/** Whether a middle-click pastes: on, and not a click the remote app is tracking. */
export function middleClickPastes(enabled: boolean, mouseTracking: boolean, shift: boolean): boolean {
  return enabled && (!mouseTracking || shift);
}

/** Ctrl+Shift+A off macOS, Cmd+Shift+A on it: copy the whole scrollback, as PuTTY's
 *  "Copy All to Clipboard". Ctrl+Shift+A has no meaning of its own to a shell. */
export function isCopyAllShortcut(e: KeyPress, mac: boolean): boolean {
  if (e.type !== 'keydown' || e.isComposing || e.keyCode === 229) return false;
  if (e.altKey || !e.shiftKey) return false;
  if (mac ? !e.metaKey || e.ctrlKey : !e.ctrlKey || e.metaKey) return false;
  return e.key === 'a' || e.key === 'A' || (e.code === 'KeyA' && nonLatinLetter(e.key));
}

/** The part of an xterm buffer `bufferText` reads, so tests can pass a plain array. */
export interface BufferLike {
  length: number;
  getLine(y: number): { isWrapped: boolean; translateToString(trimRight?: boolean): string } | undefined;
}

/** Everything in a buffer — scrollback and screen — as text. A soft-wrapped row joins
 *  the one before it rather than starting a line of its own, and the blank rows below
 *  the last output are dropped. */
export function bufferText(buffer: BufferLike, eol = '\n'): string {
  const lines: string[] = [];
  for (let y = 0; y < buffer.length; y++) {
    const line = buffer.getLine(y);
    if (!line) continue;
    // Untrimmed, so a wrap that broke at a space keeps it; the logical line is
    // trimmed once it is whole.
    const text = line.translateToString(false);
    if (line.isWrapped && lines.length) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  const trimmed = lines.map((l) => l.replace(/\s+$/, ''));
  while (trimmed.length && trimmed[trimmed.length - 1] === '') trimmed.pop();
  return trimmed.join(eol);
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
