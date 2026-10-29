import { describe, expect, it } from 'vitest';
import { CLIPBOARD_DEFAULTS, readPrefs } from './clipboard';

describe('clipboard prefs', () => {
  it('defaults to PuTTY behaviour', () => {
    expect(CLIPBOARD_DEFAULTS).toMatchObject({
      copyOnSelect: true,
      rightClick: 'paste',
      middleClickPaste: true
    });
    expect(readPrefs(undefined)).toEqual(CLIPBOARD_DEFAULTS);
  });

  it('keeps valid stored values and defaults the rest', () => {
    expect(readPrefs({ copyOnSelect: false, rightClick: 'copyOrPaste' })).toEqual({
      ...CLIPBOARD_DEFAULTS,
      copyOnSelect: false,
      rightClick: 'copyOrPaste'
    });
    expect(readPrefs({ rightClick: 'popup', middleClickPaste: 'yes' })).toEqual(CLIPBOARD_DEFAULTS);
  });
});
