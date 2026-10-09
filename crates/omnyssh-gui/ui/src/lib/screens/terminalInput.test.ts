import { describe, expect, it } from 'vitest';
import {
  chunkBytes,
  closesEndedTab,
  INPUT_CHUNK,
  isCopyShortcut,
  isFindShortcut,
  layoutFallback,
  type KeyPress
} from './terminalInput';

const seq = (n: number) => new Uint8Array(Array.from({ length: n }, (_, i) => i & 0xff));

const press = (over: Partial<KeyPress>): KeyPress => ({
  type: 'keydown',
  key: 'C',
  code: 'KeyC',
  keyCode: 67,
  ctrlKey: true,
  shiftKey: true,
  altKey: false,
  metaKey: false,
  isComposing: false,
  ...over
});

describe('isCopyShortcut — Ctrl+Shift+C copies on Windows and Linux', () => {
  it('copies on Ctrl+Shift+C, whichever case the key reports', () => {
    expect(isCopyShortcut(press({}), false)).toBe(true);
    expect(isCopyShortcut(press({ key: 'c' }), false)).toBe(true);
  });

  it('copies on the same physical key under a non-Latin layout', () => {
    // What the C key types on a Russian layout, with the keyCode WebKitGTK gives it.
    expect(isCopyShortcut(press({ key: '\u0421', keyCode: 0 }), false)).toBe(true);
  });

  it('follows the letter, not the key, on a Latin layout like Dvorak', () => {
    expect(isCopyShortcut(press({ key: 'J', code: 'KeyC' }), false)).toBe(false);
    expect(isCopyShortcut(press({ key: 'C', code: 'KeyI' }), false)).toBe(true);
    // A Latin letter with a diacritic on the C key is still that letter.
    expect(isCopyShortcut(press({ key: '\u00e7', code: 'KeyC' }), false)).toBe(false);
  });

  it('leaves bare Ctrl+C to the shell as ^C', () => {
    expect(isCopyShortcut(press({ shiftKey: false, key: 'c' }), false)).toBe(false);
  });

  it('leaves Ctrl+Shift+V, other keys and extra modifiers alone', () => {
    expect(isCopyShortcut(press({ key: 'V', code: 'KeyV' }), false)).toBe(false);
    expect(isCopyShortcut(press({ altKey: true }), false)).toBe(false);
    expect(isCopyShortcut(press({ metaKey: true }), false)).toBe(false);
  });

  it('acts on keydown only, and never mid-composition', () => {
    expect(isCopyShortcut(press({ type: 'keyup' }), false)).toBe(false);
    expect(isCopyShortcut(press({ type: 'keypress' }), false)).toBe(false);
    expect(isCopyShortcut(press({ isComposing: true }), false)).toBe(false);
    expect(isCopyShortcut(press({ key: 'Process', keyCode: 229 }), false)).toBe(false);
  });

  it('does nothing on macOS, where Cmd+C already copies', () => {
    expect(isCopyShortcut(press({}), true)).toBe(false);
  });
});

describe('isFindShortcut — ⌘F on macOS, Ctrl+Shift+F elsewhere opens find', () => {
  const find = (over: Partial<KeyPress>) => press({ key: 'f', code: 'KeyF', keyCode: 70, ...over });
  const cmdF = (over: Partial<KeyPress> = {}) =>
    find({ ctrlKey: false, shiftKey: false, metaKey: true, ...over });

  it('opens on ⌘F on macOS, whichever case the key reports', () => {
    expect(isFindShortcut(cmdF(), true)).toBe(true);
    expect(isFindShortcut(cmdF({ key: 'F' }), true)).toBe(true);
  });

  it('opens on Ctrl+Shift+F on Windows and Linux', () => {
    expect(isFindShortcut(find({ key: 'F' }), false)).toBe(true);
  });

  it('leaves a bare Ctrl+F to the shell', () => {
    expect(isFindShortcut(find({ shiftKey: false }), false)).toBe(false);
    expect(isFindShortcut(find({ shiftKey: false }), true)).toBe(false);
  });

  it('takes only its own platform chord', () => {
    expect(isFindShortcut(cmdF(), false)).toBe(false);
    expect(isFindShortcut(find({}), true)).toBe(false);
    expect(isFindShortcut(cmdF({ shiftKey: true }), true)).toBe(false);
    expect(isFindShortcut(find({ altKey: true }), false)).toBe(false);
  });

  it('follows the physical key only under a non-Latin layout', () => {
    expect(isFindShortcut(find({ key: '\u0410', keyCode: 0 }), false)).toBe(true);
    expect(isFindShortcut(find({ key: 'U', code: 'KeyF' }), false)).toBe(false);
  });

  it('never fires on keyup or mid-composition', () => {
    expect(isFindShortcut(cmdF({ type: 'keyup' }), true)).toBe(false);
    expect(isFindShortcut(cmdF({ isComposing: true }), true)).toBe(false);
    expect(isFindShortcut(find({ keyCode: 229 }), false)).toBe(false);
  });
});

describe('closesEndedTab — Enter or Esc dismisses a tab whose session ended', () => {
  const key = (k: string, over: Partial<KeyPress> = {}): KeyPress =>
    press({ key: k, code: k, keyCode: 13, ctrlKey: false, shiftKey: false, ...over });

  it('closes on a bare Enter or Esc', () => {
    expect(closesEndedTab(key('Enter'))).toBe(true);
    expect(closesEndedTab(key('Escape'))).toBe(true);
  });

  it('leaves other keys and modified chords alone', () => {
    expect(closesEndedTab(key('a'))).toBe(false);
    expect(closesEndedTab(key('Enter', { shiftKey: true }))).toBe(false);
    expect(closesEndedTab(key('Enter', { ctrlKey: true }))).toBe(false);
    expect(closesEndedTab(key('Escape', { altKey: true }))).toBe(false);
    expect(closesEndedTab(key('Enter', { metaKey: true }))).toBe(false);
  });

  it('ignores the keypress/keyup halves and IME composition', () => {
    expect(closesEndedTab(key('Enter', { type: 'keyup' }))).toBe(false);
    expect(closesEndedTab(key('Enter', { type: 'keypress' }))).toBe(false);
    // Enter that commits a composition belongs to the IME.
    expect(closesEndedTab(key('Enter', { isComposing: true }))).toBe(false);
    expect(closesEndedTab(key('Enter', { keyCode: 229 }))).toBe(false);
  });
});

// WebKitGTK under a Russian layout: the key is Cyrillic, the keyCode 0, the code Latin.
const cyrillic = (code: string, over: Partial<KeyPress> = {}): KeyPress =>
  press({ key: '\u0441', code, keyCode: 0, shiftKey: false, ...over });

describe('layoutFallback — Ctrl chords under a non-Latin layout on WebKitGTK', () => {
  it('sends the control character of the physical letter key', () => {
    expect(layoutFallback(cyrillic('KeyC'))).toEqual({ kind: 'control', data: '\x03' });
    expect(layoutFallback(cyrillic('KeyD'))).toEqual({ kind: 'control', data: '\x04' });
    expect(layoutFallback(cyrillic('KeyZ'))).toEqual({ kind: 'control', data: '\x1a' });
    expect(layoutFallback(cyrillic('KeyA'))).toEqual({ kind: 'control', data: '\x01' });
  });

  it("covers Ctrl+[ (vi's escape) and the other punctuation chords xterm knows", () => {
    expect(layoutFallback(cyrillic('BracketLeft'))).toEqual({ kind: 'control', data: '\x1b' });
    expect(layoutFallback(cyrillic('Backslash'))).toEqual({ kind: 'control', data: '\x1c' });
    expect(layoutFallback(cyrillic('BracketRight'))).toEqual({ kind: 'control', data: '\x1d' });
  });

  it('pastes on Ctrl+Shift+V, the chord the webview no longer sees', () => {
    expect(layoutFallback(cyrillic('KeyV', { shiftKey: true }))).toEqual({ kind: 'paste' });
    expect(layoutFallback(cyrillic('KeyC', { shiftKey: true }))).toBeNull();
  });

  it('stays out of the way wherever the keyCode is known', () => {
    // Latin layouts, and every other webview even under Cyrillic, report one.
    expect(layoutFallback(press({ key: 'c', shiftKey: false, keyCode: 67 }))).toBeNull();
    expect(layoutFallback(cyrillic('KeyV', { shiftKey: true, keyCode: 86 }))).toBeNull();
  });

  it('leaves a Latin layout its own letters, even the ones WebKitGTK cannot name', () => {
    // German Ü on the [ key, Spanish Ç on the \ key, a dead key, Turkish dotless i:
    // keyCode 0 there too, but the chord must not become ESC, SIGQUIT's FS or ^I.
    expect(layoutFallback(cyrillic('BracketLeft', { key: '\u00fc' }))).toBeNull();
    expect(layoutFallback(cyrillic('Backslash', { key: '\u00e7' }))).toBeNull();
    expect(layoutFallback(cyrillic('BracketRight', { key: 'Dead' }))).toBeNull();
    expect(layoutFallback(cyrillic('KeyI', { key: '\u0131' }))).toBeNull();
    expect(layoutFallback(cyrillic('KeyV', { key: '\u00dc', shiftKey: true }))).toBeNull();
  });

  it('works for any non-Latin script, not just Cyrillic', () => {
    // Greek sigma on the C key.
    expect(layoutFallback(cyrillic('KeyC', { key: '\u03c3' }))).toEqual({
      kind: 'control',
      data: '\x03'
    });
  });

  it('leaves plain keys, other modifiers, key-up and compositions alone', () => {
    expect(layoutFallback(cyrillic('KeyC', { ctrlKey: false }))).toBeNull();
    expect(layoutFallback(cyrillic('KeyC', { altKey: true }))).toBeNull();
    expect(layoutFallback(cyrillic('KeyC', { metaKey: true }))).toBeNull();
    expect(layoutFallback(cyrillic('KeyC', { type: 'keyup' }))).toBeNull();
    expect(layoutFallback(cyrillic('KeyC', { isComposing: true }))).toBeNull();
    expect(layoutFallback(cyrillic('Digit1'))).toBeNull();
  });
});

describe('chunkBytes — bounded terminal input', () => {
  it('yields nothing for empty input', () => {
    expect(chunkBytes(new Uint8Array(0))).toEqual([]);
  });

  it('yields a single slice for input at or below the cap (the keystroke path)', () => {
    expect(chunkBytes(seq(1), 8)).toHaveLength(1);
    expect(chunkBytes(seq(8), 8)).toHaveLength(1);
    expect(chunkBytes(seq(8), 8)[0]).toHaveLength(8);
  });

  it('splits a large paste into ordered <=size slices that reassemble to the input', () => {
    const data = seq(21);
    const chunks = chunkBytes(data, 8);
    expect(chunks.map((c) => c.length)).toEqual([8, 8, 5]);
    expect(chunks.every((c) => c.length <= 8)).toBe(true);
    expect(new Uint8Array(chunks.flatMap((c) => [...c]))).toEqual(data);
  });

  it('splits an exact multiple into full slices only', () => {
    expect(chunkBytes(seq(16), 8).map((c) => c.length)).toEqual([8, 8]);
  });

  it('defaults to the INPUT_CHUNK cap', () => {
    expect(chunkBytes(seq(INPUT_CHUNK + 1)).map((c) => c.length)).toEqual([INPUT_CHUNK, 1]);
  });
});
