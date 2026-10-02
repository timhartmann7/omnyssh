import { expect, test, type Page } from '@playwright/test';

// Encrypted identity files (issue #97, tech-gui.md §4.2/§4.3). e2e runs against the
// static SPA with Tauri absent, so `__TAURI_INTERNALS__` is stubbed at the boundary
// (§6.4). The stub plays the core: `key-passphrase-required` names a locked key,
// `unlock_identity` accepts only the right passphrase, and every call is recorded.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: [], source: 'manual', hasKey: true, localForwards: [], tunnelAutostart: false, forwardAgent: false },
  { name: 'web-2', hostname: 'web-2.example.com', user: 'deploy', port: 22, tags: [], source: 'manual', hasKey: true, localForwards: [], tunnelAutostart: false, forwardAgent: false }
];
const KEY = '/home/me/.ssh/id_ed25519';
const OTHER = '/home/me/.ssh/deploy_key';

type Unlock = { keyPath: string; passphrase: string };

type Terminal = 'locked' | 'slow' | 'ok';

async function boot(
  page: Page,
  opts: { lockedAtLaunch: boolean; terminal?: Terminal; unlockDelay?: number }
): Promise<void> {
  await page.addInitScript(
    ({ hosts, key, other, lockedAtLaunch, terminal, unlockDelay }) => {
      let cbid = 0;
      const win = window as unknown as Record<string, unknown>;
      const listeners: Record<string, number[]> = {};
      const unlocks: Array<{ keyPath: string; passphrase: string }> = [];
      win.__unlocks = unlocks;

      function fire(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          const cb = win[`__cb${id}`] as ((e: unknown) => void) | undefined;
          cb?.({ event, id, payload });
        }
      }
      win.__fire = fire;

      (win as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve(hosts);
            case 'reload_hosts':
              if (lockedAtLaunch) {
                // Both pollers hit the same locked key; a second key waits behind it.
                setTimeout(() => {
                  fire('key-passphrase-required', { hostName: 'web-1', keyPath: key });
                  fire('key-passphrase-required', { hostName: 'web-2', keyPath: key });
                  fire('key-passphrase-required', { hostName: 'web-2', keyPath: other });
                }, 0);
              }
              return Promise.resolve(null);
            case 'unlock_identity': {
              const { keyPath, passphrase } = args as { keyPath: string; passphrase: string };
              unlocks.push({ keyPath, passphrase });
              return new Promise((resolve, reject) =>
                setTimeout(
                  () => (passphrase === 'sesame' ? resolve(null) : reject({ message: 'wrong passphrase' })),
                  unlockDelay
                )
              );
            }
            case 'terminal_open': {
              if (terminal === 'ok') return Promise.resolve(7);
              if (terminal === 'slow') {
                // The key locks another tab while this one is still opening.
                setTimeout(() => fire('key-passphrase-required', { hostName: 'web-2', keyPath: key }), 0);
                return new Promise((resolve) =>
                  setTimeout(() => {
                    win.__opened = true;
                    resolve(7);
                  }, 600)
                );
              }
              // What the core sends when the terminal's key is locked.
              setTimeout(() => {
                fire('error', { message: `Terminal: SSH key requires a passphrase: ${key}` });
                fire('key-passphrase-required', { hostName: 'web-1', keyPath: key });
              }, 30);
              setTimeout(() => fire('terminal-exited', { sessionId: 1, hadOutput: false }), 400);
              return Promise.resolve(1);
            }
            case 'plugin:event|listen': {
              const { event, handler } = args as { event: string; handler: number };
              (listeners[event] ||= []).push(handler);
              return Promise.resolve(cbid);
            }
            default:
              return Promise.resolve(null);
          }
        },
        transformCallback: (cb: unknown) => {
          const id = ++cbid;
          win[`__cb${id}`] = cb;
          return id;
        },
        unregisterCallback: (id: number) => {
          delete win[`__cb${id}`];
        }
      };
    },
    {
      hosts: HOSTS,
      key: KEY,
      other: OTHER,
      lockedAtLaunch: opts.lockedAtLaunch,
      terminal: opts.terminal ?? 'locked',
      unlockDelay: opts.unlockDelay ?? 0
    }
  );
  await page.goto('/');
  await expect(page.getByText('2 hosts')).toBeVisible();
}

const unlocks = (page: Page): Promise<Unlock[]> =>
  page.evaluate(() => (window as unknown as { __unlocks: Unlock[] }).__unlocks);

test('a key locked at launch is asked for once, keeps typing across repeats, and moves on', async ({
  page
}) => {
  await boot(page, { lockedAtLaunch: true });

  const dialog = page.getByRole('dialog', { name: 'Unlock SSH key' });
  await expect(dialog).toHaveCount(1);
  await expect(dialog.getByText('Unlock SSH key — web-1')).toBeVisible();
  await expect(dialog.getByText(KEY)).toBeVisible();
  const input = dialog.getByLabel('Passphrase');
  await expect(input).toBeFocused();
  await expect(dialog.getByRole('button', { name: 'Unlock' })).toBeDisabled();

  // Another host reporting the same key must not wipe what is being typed.
  await input.fill('wro');
  await page.evaluate((key) => {
    (window as unknown as { __fire: (e: string, p: unknown) => void }).__fire(
      'key-passphrase-required',
      { hostName: 'web-2', keyPath: key }
    );
  }, KEY);
  await expect(input).toHaveValue('wro');

  // A wrong passphrase keeps the dialog up, says so, and clears the field.
  await input.fill('wrong');
  await input.press('Enter');
  await expect(dialog.getByText('wrong passphrase')).toBeVisible();
  await expect(input).toHaveValue('');

  await input.fill('sesame');
  await input.press('Enter');
  // One unlock served both hosts on that key; the other key is next.
  await expect(dialog.getByText(OTHER)).toBeVisible();
  await expect(dialog.getByText('Unlock SSH key — web-2')).toBeVisible();
  await expect(dialog.getByText('wrong passphrase')).toHaveCount(0);
  expect(await unlocks(page)).toEqual([
    { keyPath: KEY, passphrase: 'wrong' },
    { keyPath: KEY, passphrase: 'sesame' }
  ]);

  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('a terminal on a locked key closes with the reason and asks for the passphrase', async ({
  page
}) => {
  await boot(page, { lockedAtLaunch: false });

  await page.getByTitle('sh on web-1').click();
  const tab = page.getByRole('button', { name: 'web-1 · terminal', exact: true });
  await expect(tab).toBeVisible();

  const dialog = page.getByRole('dialog', { name: 'Unlock SSH key' });
  await expect(dialog.getByText(KEY)).toBeVisible();
  await expect(page.getByText(`Terminal: SSH key requires a passphrase: ${KEY}`)).toBeVisible();
  await expect(tab).toHaveCount(0);

  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('a terminal that finishes opening under the dialog leaves the keyboard to it', async ({
  page
}) => {
  await boot(page, { lockedAtLaunch: false, terminal: 'slow' });

  await page.getByTitle('sh on web-1').click();
  const input = page.getByRole('dialog', { name: 'Unlock SSH key' }).getByLabel('Passphrase');
  await expect(input).toBeFocused();

  // Once the terminal is ready (plus a frame), typing must still land in the dialog.
  await page.waitForFunction(() => (window as unknown as { __opened?: boolean }).__opened);
  await page.waitForTimeout(100);
  await page.keyboard.type('sesame');
  await expect(input).toBeFocused();
  await expect(input).toHaveValue('sesame');

  // Once the dialog goes, the terminal gets the keyboard it was kept from.
  await page.keyboard.press('Escape');
  await expect(page.locator('.xterm-helper-textarea')).toBeFocused();
});

test('closing the dialog hands the keyboard back to the terminal it covered', async ({ page }) => {
  await boot(page, { lockedAtLaunch: false, terminal: 'ok' });

  await page.getByTitle('sh on web-1').click();
  const terminalInput = page.locator('.xterm-helper-textarea');
  await expect(terminalInput).toBeFocused();

  await page.evaluate((key) => {
    (window as unknown as { __fire: (e: string, p: unknown) => void }).__fire(
      'key-passphrase-required',
      { hostName: 'web-2', keyPath: key }
    );
  }, KEY);
  await expect(page.getByRole('dialog', { name: 'Unlock SSH key' }).getByLabel('Passphrase')).toBeFocused();

  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(terminalInput).toBeFocused();
});

test('a prompt that opens just as a terminal comes back keeps the keyboard', async ({ page }) => {
  await boot(page, { lockedAtLaunch: false, terminal: 'ok' });

  await page.getByTitle('sh on web-1').click();
  await expect(page.locator('.xterm-helper-textarea')).toBeFocused();
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click();

  // Showing the tab queues the terminal's focus for the next frame; the prompt opens
  // before that frame, in the same task.
  await page.evaluate(async (key) => {
    document.querySelector<HTMLButtonElement>('button[aria-label="web-1 · terminal"]')?.click();
    for (let i = 0; i < 5; i++) await Promise.resolve();
    (window as unknown as { __fire: (e: string, p: unknown) => void }).__fire(
      'key-passphrase-required',
      { hostName: 'web-2', keyPath: key }
    );
  }, KEY);
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));

  await expect(page.getByRole('dialog', { name: 'Unlock SSH key' }).getByLabel('Passphrase')).toBeFocused();
});

test('cancelling while an unlock is running leaves the next key alone', async ({ page }) => {
  await boot(page, { lockedAtLaunch: true, unlockDelay: 500 });

  const dialog = page.getByRole('dialog', { name: 'Unlock SSH key' });
  const input = dialog.getByLabel('Passphrase');
  await input.fill('wrong');
  await input.press('Enter');
  // The first key's unlock is still running when the user gives up on it.
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog.getByText(OTHER)).toBeVisible();

  await page.waitForTimeout(700);
  await expect(dialog.getByText(OTHER)).toBeVisible();
  await expect(dialog.getByText('wrong passphrase')).toHaveCount(0);
});

test('Escape on the prompt closes only the prompt, not the form under it', async ({ page }) => {
  await boot(page, { lockedAtLaunch: false });

  await page.getByRole('button', { name: 'Add host' }).click();
  const editor = page.getByRole('dialog', { name: 'Add host' });
  const name = editor.getByLabel('Name', { exact: true });
  await name.fill('db-9');

  await page.evaluate((key) => {
    (window as unknown as { __fire: (e: string, p: unknown) => void }).__fire(
      'key-passphrase-required',
      { hostName: 'web-2', keyPath: key }
    );
  }, KEY);
  const prompt = page.getByRole('dialog', { name: 'Unlock SSH key' });
  await expect(prompt.getByLabel('Passphrase')).toBeFocused();

  await page.keyboard.press('Escape');
  await expect(prompt).toHaveCount(0);
  await expect(editor).toBeVisible();
  await expect(name).toHaveValue('db-9');
  // Focus goes back to the field the prompt interrupted.
  await expect(name).toBeFocused();
});
