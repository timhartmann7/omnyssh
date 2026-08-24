import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

// A bare `<select>` is drawn by the platform, not by the theme: WebKit paints macOS's
// popup button over any colour utility on it, so one dropped straight into a form is a
// bright native control in the middle of the app's own fields. The fix is structural —
// route every select through `Select.svelte`, which strips that chrome — so guard it
// structurally too, the way the colour-token rule is guarded.
const SRC = fileURLToPath(new URL('../..', import.meta.url));
const COMPONENT = 'lib/components/Select.svelte';

function svelteFiles(): string[] {
  return readdirSync(SRC, { recursive: true, encoding: 'utf8' })
    .filter((f) => f.endsWith('.svelte'))
    .map((f) => join(SRC, f));
}

describe('select controls', () => {
  it('only Select.svelte renders a raw <select>', () => {
    const files = svelteFiles();
    expect(files.length).toBeGreaterThan(0);

    const offenders = files.filter(
      (file) =>
        !file.replaceAll('\\', '/').endsWith(COMPONENT) &&
        readFileSync(file, 'utf8').includes('<select')
    );
    expect(offenders).toEqual([]);
  });

  it('Select.svelte strips the native control chrome', () => {
    const source = readFileSync(join(SRC, COMPONENT), 'utf8');

    // Without this the platform draws its own control and every colour utility on the
    // element is ignored.
    expect(source).toMatch(/<select[^>]*appearance-none/s);
    // The chevron replaces the native one, so it has to stay out of the hit area and
    // take its colour from the theme rather than a literal.
    expect(source).toContain('pointer-events-none');
    expect(source).toContain('stroke="currentColor"');
  });
});
