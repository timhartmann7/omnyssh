import { expect, test, type Page } from '@playwright/test';

// The ⌘K palette and the host-picker (tech-gui.md §2, Stage 1.3). e2e runs against the
// static SPA; Tauri is absent, so we stub `__TAURI_INTERNALS__` to feed `list_hosts`
// fixtures into the store — the same seam §6.4 reserves for mocking IPC at the boundary.
const HOSTS = [
  {
    name: 'web-1',
    hostname: 'web-1.example.com',
    user: 'deploy',
    port: 22,
    tags: ['prod'],
    source: 'manual',
    hasKey: true,
    localForwards: [],
    tunnelAutostart: false,
    forwardAgent: false
  },
  {
    name: 'db-1',
    hostname: 'db-1.example.com',
    user: 'root',
    port: 22,
    tags: [],
    source: 'manual',
    hasKey: false,
    localForwards: [],
    tunnelAutostart: false,
    forwardAgent: false
  }
];

async function bootWithHosts(page: Page, hosts = HOSTS): Promise<void> {
  await page.addInitScript((hosts) => {
    let cbid = 0;
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: (cmd: string) =>
        cmd === 'list_hosts' ? Promise.resolve(hosts) : Promise.resolve(null),
      transformCallback: (cb: unknown) => {
        const id = ++cbid;
        (window as unknown as Record<string, unknown>)[`__cb${id}`] = cb;
        return id;
      }
    };
  }, hosts);
  await page.goto('/');
  // The status-bar total confirms `list_hosts` resolved into the store.
  await expect(page.getByText(`${hosts.length} hosts`)).toBeVisible();
}

test('⌘K navigator jumps to a host by opening a session', async ({ page }) => {
  await bootWithHosts(page);

  await page.keyboard.press('Control+k');
  const dialog = page.getByRole('dialog', { name: 'Command palette' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('textbox')).toBeFocused();

  await page.keyboard.type('web');
  await page.keyboard.press('Enter');

  await expect(page.getByRole('dialog')).toHaveCount(0);
  const row = page.getByRole('button', { name: 'web-1 · terminal', exact: true });
  await expect(row).toBeVisible();
  await expect(row).toHaveAttribute('aria-current', 'true');
});

test('⌘K toggles the navigator open and closed', async ({ page }) => {
  await bootWithHosts(page);

  await page.keyboard.press('Control+k');
  await expect(page.getByRole('dialog', { name: 'Command palette' })).toBeVisible();

  await page.keyboard.press('Control+k');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('a spawner opens the host-picker and spawns a session for the chosen host', async ({
  page
}) => {
  await bootWithHosts(page);

  await page.getByRole('button', { name: 'Terminal' }).click();
  const picker = page.getByRole('dialog', { name: 'Pick a host' });
  await expect(picker).toBeVisible();

  await picker.getByRole('button', { name: /db-1/ }).click();

  await expect(page.getByRole('dialog')).toHaveCount(0);
  const row = page.getByRole('button', { name: 'db-1 · terminal', exact: true });
  await expect(row).toBeVisible();
  await expect(row).toHaveAttribute('aria-current', 'true');
});

test('lists a shared host name once, as the host a session opens', async ({ page }) => {
  const twin = { ...HOSTS[0], hostname: 'web-1b.example.com', user: 'admin' };
  await bootWithHosts(page, [...HOSTS, twin]);

  await page.getByRole('button', { name: 'Terminal' }).click();
  const picker = page.getByRole('dialog', { name: 'Pick a host' });
  const web = picker.getByRole('button', { name: /web-1/ });
  await expect(web).toHaveCount(1);
  await expect(web).toContainText('deploy@web-1.example.com');
  await picker.getByRole('textbox').fill('web-1b');
  await expect(picker.getByText('No matching hosts.')).toBeVisible();
  await page.keyboard.press('Escape');

  await page.keyboard.press('Control+k');
  const listed = page
    .getByRole('dialog', { name: 'Command palette' })
    .getByRole('button', { name: /web-1/ });
  await expect(listed).toHaveCount(1);
  await expect(listed).toContainText('deploy@web-1.example.com');
});
