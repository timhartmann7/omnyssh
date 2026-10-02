import { expect, test, type Page } from '@playwright/test';

// Host management CRUD (tech-gui.md §4.1). e2e runs against the static SPA; Tauri is
// absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). The stub is
// stateful: save/delete mutate an in-memory list, and `reload_hosts` replays it as a
// `hosts-loaded` event through the same listener the app registers — so a save/delete
// round-trips into the dashboard grid exactly as the real backend would drive it.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: ['prod'], source: 'manual', hasKey: true, localForwards: [], tunnelAutostart: false, forwardAgent: false },
  { name: 'imported', hostname: 'imported.example.com', user: 'root', port: 22, tags: [], source: 'sshConfig', hasKey: false, localForwards: [], tunnelAutostart: false, forwardAgent: false },
  { name: 'api-1', hostname: 'api-1.example.com', user: 'deploy', port: 22, tags: ['prod', 'api'], source: 'manual', hasKey: true, localForwards: [], tunnelAutostart: false, forwardAgent: false }
];

async function boot(page: Page, seed: Array<Record<string, unknown>> = HOSTS): Promise<void> {
  await page.addInitScript(
    ({ hosts }) => {
      let cbid = 0;
      const listeners: Record<string, number[]> = {};
      const state: { hosts: Array<Record<string, unknown>> } = { hosts: hosts.map((h) => ({ ...h })) };
      const win = window as unknown as Record<string, unknown>;

      function fire(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          const cb = win[`__cb${id}`] as ((e: unknown) => void) | undefined;
          cb?.({ event, id, payload });
        }
      }

      (win as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve([...state.hosts]);
            case 'reload_hosts':
              // The real command reloads + restarts pollers, then broadcasts the list.
              setTimeout(() => fire('hosts-loaded', [...state.hosts]), 0);
              return Promise.resolve(null);
            case 'save_host': {
              // Upsert by name as a manual host; the outbound view (HostDto) omits the
              // secret fields the input carried, mirroring the backend map (§3.4).
              const h = args.input as Record<string, unknown> & { name: string; identityFile?: string };
              const view = {
                name: h.name,
                hostname: h.hostname,
                user: h.user,
                port: h.port,
                tags: (h.tags as string[]) ?? [],
                notes: h.notes,
                source: 'manual',
                hasKey: !!h.identityFile,
                localForwards: h.localForwards,
                tunnelAutostart: h.tunnelAutostart,
                forwardAgent: h.forwardAgent
              };
              const i = state.hosts.findIndex((x) => (x as { name: string }).name === view.name);
              if (i >= 0) state.hosts[i] = { ...state.hosts[i], ...view };
              else state.hosts.push(view);
              return Promise.resolve(null);
            }
            case 'delete_host':
              state.hosts = state.hosts.filter((x) => (x as { name: string }).name !== args.name);
              return Promise.resolve(null);
            case 'list_snippets':
              return Promise.resolve([]);
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
        }
      };
    },
    { hosts: seed }
  );
  await page.goto('/');
  // The dashboard is the default screen; the seeded cards confirm the app booted.
  await expect(page.getByText(String(seed[0].name), { exact: true }).first()).toBeVisible();
}

test('adds a host and it appears as a card', async ({ page }) => {
  await boot(page);

  await page.getByRole('button', { name: 'Add host' }).click();
  const editor = page.getByRole('dialog', { name: 'Add host' });
  await expect(editor).toBeVisible();

  await editor.getByLabel('Name', { exact: true }).fill('db-1');
  await editor.getByLabel('Hostname / IP').fill('db-1.example.com');
  await editor.getByLabel('User').fill('postgres');
  await editor.getByRole('button', { name: 'Add host' }).click();

  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByText('db-1', { exact: true })).toBeVisible();
  await expect(page.getByText('postgres@db-1.example.com:22')).toBeVisible();
});

test('edits a manual host in place', async ({ page }) => {
  await boot(page);

  await page.getByRole('button', { name: 'Edit web-1' }).click();
  const editor = page.getByRole('dialog', { name: 'Edit host' });
  await expect(editor).toBeVisible();

  // The name is the on-disk key: fixed on edit.
  await expect(editor.getByLabel(/^Name/)).toHaveAttribute('readonly', '');

  const hostname = editor.getByLabel('Hostname / IP');
  await hostname.fill('web-1b.example.com');
  await editor.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByText('deploy@web-1b.example.com:22')).toBeVisible();
});

test.describe('on Linux', () => {
  test.use({ userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)' });

  test('agent forwarding is off until switched on, and stays on', async ({ page }) => {
    await boot(page);

    await page.getByRole('button', { name: 'Edit web-1' }).click();
    let editor = page.getByRole('dialog', { name: 'Edit host' });
    const agent = editor.getByRole('switch', { name: 'Forward SSH agent' });
    await expect(agent).toHaveAttribute('aria-checked', 'false');
    await agent.click();
    await editor.getByRole('button', { name: 'Save' }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);

    await page.getByRole('button', { name: 'Edit web-1' }).click();
    editor = page.getByRole('dialog', { name: 'Edit host' });
    await expect(editor.getByRole('switch', { name: 'Forward SSH agent' })).toHaveAttribute(
      'aria-checked',
      'true'
    );
  });
});

// The Desktop Chrome device reports a Windows user agent.
test('agent forwarding says it is not on Windows yet', async ({ page }) => {
  await boot(page);

  await page.getByRole('button', { name: 'Edit web-1' }).click();
  const editor = page.getByRole('dialog', { name: 'Edit host' });
  await expect(editor.getByRole('switch', { name: 'Forward SSH agent' })).toBeDisabled();
  await expect(editor.getByText('Not available on Windows yet.')).toBeVisible();
});

test('deletes a manual host after confirmation', async ({ page }) => {
  await boot(page);
  await expect(page.getByText('web-1', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Delete web-1' }).click();
  const confirm = page.getByRole('dialog', { name: 'Delete host' });
  await expect(confirm).toBeVisible();
  await confirm.getByRole('button', { name: 'Delete', exact: true }).click();

  await expect(page.getByText('web-1', { exact: true })).toHaveCount(0);
});

test('an SSH-config host is adopted by editing it', async ({ page }) => {
  await boot(page);

  // The import is marked, and there is nothing here to delete: it lives in
  // ~/.ssh/config, which this app never writes.
  await expect(page.getByText('ssh config')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete imported' })).toHaveCount(0);

  await page.getByRole('button', { name: 'Edit imported' }).click();
  const editor = page.getByRole('dialog', { name: 'Edit host' });
  await expect(editor).toBeVisible();
  // The form states what saving does, since the SSH config file itself does not change.
  await expect(editor.getByText(/never written/)).toBeVisible();

  await editor.getByLabel('Hostname / IP').fill('adopted.example.com');
  await editor.getByRole('button', { name: 'Save' }).click();

  // Saved as a manual copy: the import badge is gone and delete is now offered.
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByText('root@adopted.example.com:22')).toBeVisible();
  await expect(page.getByText('ssh config')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Delete imported' })).toHaveCount(1);
});

test('View options groups hosts under their first tag', async ({ page }) => {
  await boot(page);
  const trigger = page.getByRole('button', { name: 'View options' });
  const dot = trigger.locator('span.bg-accent');
  await expect(dot).toHaveCount(0);

  await trigger.focus();
  await page.keyboard.press('Enter');
  const panel = page.getByRole('dialog', { name: 'View options' });
  // Opaque: with no scrim, cards behind must not show through where blur isn't drawn.
  expect(await panel.evaluate((el) => getComputedStyle(el).backgroundColor)).toMatch(/^rgb\(/);
  const grouping = panel.getByRole('switch', { name: 'Group by tag' });
  await expect(grouping).toBeFocused();
  await page.keyboard.press('Space');
  await expect(grouping).toHaveAttribute('aria-checked', 'true');
  await expect(dot).toHaveCount(1);

  // api-1 is tagged "prod, api": one card, under its first tag only.
  const prod = page.getByRole('region', { name: 'prod hosts' });
  await expect(prod.getByText('api-1', { exact: true })).toBeVisible();
  await expect(prod.getByText('web-1', { exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'api hosts' })).toHaveCount(0);
  await expect(page.getByText('api-1', { exact: true })).toHaveCount(1);
  await expect(page.getByRole('heading', { level: 2 })).toHaveText([/prod/, /Untagged/]);

  await page.keyboard.press('Escape');
  await expect(panel).toHaveCount(0);
  await expect(trigger).toBeFocused();

  // A press outside closes it too.
  await trigger.click();
  await expect(panel).toBeVisible();
  await page.getByRole('heading', { name: 'Dashboard' }).click();
  await expect(panel).toHaveCount(0);

  // `g` switches grouping back off.
  await page.keyboard.press('g');
  await expect(prod).toHaveCount(0);
  await expect(dot).toHaveCount(0);
});

test('folded sections stay folded across navigation and reload', async ({ page }) => {
  await boot(page);
  await page.keyboard.press('g');
  const header = () =>
    page.getByRole('region', { name: 'prod hosts' }).getByRole('button', { name: /^prod/ });

  await header().click();
  await expect(header()).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByText('web-1', { exact: true })).toBeHidden();
  await expect(page.getByText('imported', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Snippets', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Snippets' })).toBeVisible();
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click();
  await expect(header()).toHaveAttribute('aria-expanded', 'false');

  await page.reload();
  await expect(header()).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByText('web-1', { exact: true })).toBeHidden();

  // A search shows matches inside a folded section, and the fold returns once it clears.
  await page.getByRole('button', { name: 'Search hosts' }).click();
  const search = page.getByRole('textbox', { name: 'Search hosts' });
  await search.fill('web');
  await expect(page.getByText('web-1', { exact: true })).toBeVisible();
  await expect(header()).toBeDisabled();
  await search.press('Escape');
  await expect(page.getByText('web-1', { exact: true })).toBeHidden();

  await header().focus();
  await page.keyboard.press('Enter');
  await expect(header()).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByText('web-1', { exact: true })).toBeVisible();
});

test('a host with a repeated tag renders once', async ({ page }) => {
  const errors: Error[] = [];
  page.on('pageerror', (e) => errors.push(e));
  await boot(page);

  await page.getByRole('button', { name: 'Edit web-1' }).click();
  const editor = page.getByRole('dialog', { name: 'Edit host' });
  await expect(editor.getByText('The first tag groups the host on the dashboard.')).toBeVisible();
  await editor.getByLabel('Tags').fill('prod, prod');
  await editor.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);

  await page.keyboard.press('g');
  const prod = page.getByRole('region', { name: 'prod hosts' });
  await expect(prod.getByText('web-1', { exact: true })).toHaveCount(1);
  await expect(prod.getByText('api-1', { exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});

test('g is ignored under a modal and while typing', async ({ page }) => {
  await boot(page);
  const grouped = page.getByRole('region', { name: 'Untagged hosts' });

  // The Support dialog is not the dashboard's own: g must not regroup the grid behind it.
  await page.getByRole('button', { name: 'Support OmnySSH' }).click();
  await expect(page.getByRole('dialog', { name: 'Support OmnySSH' })).toBeVisible();
  await page.keyboard.press('g');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);

  // Nor behind the command palette, even with focus off its input.
  await page.keyboard.press('Control+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await palette.getByText('esc close').click();
  await page.keyboard.press('g');
  await expect(grouped).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(palette).toHaveCount(0);

  await page.getByRole('button', { name: 'Search hosts' }).click();
  await page.getByRole('textbox', { name: 'Search hosts' }).press('g');
  await expect(page.getByRole('textbox', { name: 'Search hosts' })).toHaveValue('g');
  await expect(grouped).toHaveCount(0);

  // Away from both, the same key groups.
  await page.getByRole('textbox', { name: 'Search hosts' }).press('Escape');
  await page.keyboard.press('g');
  await expect(grouped).toBeVisible();
});

// Card order as the grips name it, top-left to bottom-right.
async function cardOrder(page: Page): Promise<string[]> {
  const grips = page.getByRole('button', { name: /^Move / });
  return (await grips.evaluateAll((els) => els.map((el) => el.getAttribute('aria-label') ?? ''))).map(
    (label) => label.slice('Move '.length)
  );
}

// Drags a card by its grip onto the left half of another card, in small steps.
async function dragOnto(page: Page, host: string, onto: string): Promise<void> {
  await page.getByText(host, { exact: true }).hover();
  const grip = await page.getByRole('button', { name: `Move ${host}` }).boundingBox();
  const target = await page.getByText(onto, { exact: true }).boundingBox();
  if (!grip || !target) throw new Error('card not on screen');
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
  await page.mouse.down();
  await page.mouse.move(target.x + 2, target.y + target.height / 2, { steps: 12 });
  await page.mouse.up();
}

test('View options sorts by name and status with the arrow keys', async ({ page }) => {
  await boot(page);
  await expect.poll(() => cardOrder(page)).toEqual(['web-1', 'imported', 'api-1']);

  await page.getByRole('button', { name: 'View options' }).click();
  const sort = page.getByRole('radiogroup', { name: 'Sort' });
  await expect(sort.getByRole('radio', { name: 'Custom' })).toHaveAttribute('aria-checked', 'true');
  await sort.getByRole('radio', { name: 'Custom' }).focus();
  await page.keyboard.press('ArrowRight');
  const byName = sort.getByRole('radio', { name: 'Name' });
  await expect(byName).toHaveAttribute('aria-checked', 'true');
  await expect(byName).toBeFocused();
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'imported', 'web-1']);

  await page.keyboard.press('ArrowRight');
  await expect(sort.getByRole('radio', { name: 'Status' })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('ArrowRight');
  await expect(sort.getByRole('radio', { name: 'Custom' })).toHaveAttribute('aria-checked', 'true');

  await byName.click();
  await page.reload();
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'imported', 'web-1']);
});

test('dragging a card by its grip reorders the grid and persists', async ({ page }) => {
  await boot(page);
  await dragOnto(page, 'api-1', 'web-1');
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'web-1', 'imported']);
  await expect(page.locator('[aria-live="polite"]', { hasText: 'api-1 moved to position 1 of 3' })).toHaveCount(1);

  await page.reload();
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'web-1', 'imported']);

  // Escape mid-drag, or a release outside the grid, changes nothing.
  const grip = await page.getByRole('button', { name: 'Move imported' }).boundingBox();
  if (!grip) throw new Error('grip not on screen');
  await page.mouse.move(grip.x + 5, grip.y + 5);
  await page.mouse.down();
  await page.mouse.move(grip.x - 300, grip.y + 5, { steps: 8 });
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await page.mouse.move(grip.x + 5, grip.y + 5);
  await page.mouse.down();
  await page.mouse.move(grip.x + 5, 10, { steps: 8 });
  await page.mouse.up();
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'web-1', 'imported']);
});

test('a drag while sorted by name starts from the name order', async ({ page }) => {
  await boot(page);
  const options = page.getByRole('button', { name: 'View options' });
  await options.click();
  await page.getByRole('radio', { name: 'Name' }).click();
  await page.keyboard.press('Escape');
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'imported', 'web-1']);

  // A cancelled drag leaves the sort as it was.
  const grip = await page.getByRole('button', { name: 'Move web-1' }).boundingBox();
  if (!grip) throw new Error('grip not on screen');
  await page.mouse.move(grip.x + 5, grip.y + 5);
  await page.mouse.down();
  await page.mouse.move(grip.x - 300, grip.y + 5, { steps: 8 });
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await options.click();
  await expect(page.getByRole('radio', { name: 'Name' })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('Escape');

  await dragOnto(page, 'web-1', 'imported');
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'web-1', 'imported']);
  await options.click();
  await expect(page.getByRole('radio', { name: 'Custom' })).toHaveAttribute('aria-checked', 'true');
});

test('the grip moves a card with the keyboard, within its section when grouped', async ({ page }) => {
  await boot(page);
  const grip = page.getByRole('button', { name: 'Move web-1' });
  await grip.focus();
  await page.keyboard.press('ArrowRight');
  await expect.poll(() => cardOrder(page)).toEqual(['imported', 'web-1', 'api-1']);
  await expect(grip).toBeFocused();
  await expect(page.locator('[aria-live="polite"]')).toHaveText('web-1 moved to position 2 of 3');
  await page.keyboard.press('End');
  await expect.poll(() => cardOrder(page)).toEqual(['imported', 'api-1', 'web-1']);
  await page.keyboard.press('Home');
  await expect.poll(() => cardOrder(page)).toEqual(['web-1', 'imported', 'api-1']);
  await expect(grip).toBeFocused();

  // Grouped, prod holds web-1 and api-1: End moves web-1 to the end of prod only. The
  // regroup rebuilds the cards but keeps focus on the same grip.
  await page.keyboard.press('g');
  await expect.poll(() => cardOrder(page)).toEqual(['web-1', 'api-1', 'imported']);
  await expect(page.getByRole('button', { name: 'Move web-1' })).toBeFocused();
  await page.keyboard.press('End');
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'web-1', 'imported']);
  await expect(page.locator('[aria-live="polite"]')).toHaveText('web-1 moved to position 2 of 2');

  // A drag into another section is outside the run, so it does nothing.
  await dragOnto(page, 'web-1', 'imported');
  await expect.poll(() => cardOrder(page)).toEqual(['api-1', 'web-1', 'imported']);
});

test('holding a drag past the bottom edge scrolls and drops at the end', async ({ page }) => {
  await page.setViewportSize({ width: 600, height: 560 });
  await boot(page);
  await page.getByText('web-1', { exact: true }).hover();
  const grip = await page.getByRole('button', { name: 'Move web-1' }).boundingBox();
  if (!grip) throw new Error('grip not on screen');
  await page.mouse.move(grip.x + 5, grip.y + 5);
  await page.mouse.down();
  // The very bottom row of the window sits below the scrolling grid.
  await page.mouse.move(grip.x + 5, 559, { steps: 10 });
  const marker = page.locator('.pointer-events-none.fixed.bg-accent');
  await expect(marker).toBeVisible();
  // Let the auto-scroll run to the end of the grid.
  const scrolled = () =>
    page.evaluate(() => {
      let p = document.querySelector('[data-cards]')?.parentElement;
      while (p && !/auto|scroll/.test(getComputedStyle(p).overflowY)) p = p.parentElement;
      return p?.scrollTop ?? 0;
    });
  await expect
    .poll(async () => {
      const before = await scrolled();
      await page.waitForTimeout(150);
      return before > 0 && (await scrolled()) === before;
    })
    .toBe(true);
  await page.mouse.up();
  await expect.poll(() => cardOrder(page)).toEqual(['imported', 'api-1', 'web-1']);
});

test('same-named hosts keep their places while a drag starts', async ({ page }) => {
  const twin = HOSTS[1];
  await boot(page, [
    { ...twin, name: 'dup', hostname: 'dup-a.example.com' },
    { ...twin, name: 'x', hostname: 'x.example.com' },
    { ...twin, name: 'dup', hostname: 'dup-b.example.com' }
  ]);
  const addresses = () =>
    page.getByText(/^root@.*:22$/).evaluateAll((els) => els.map((el) => el.textContent?.trim()));
  const before = ['root@dup-a.example.com:22', 'root@x.example.com:22', 'root@dup-b.example.com:22'];
  await expect.poll(addresses).toEqual(before);

  await page.getByText('x', { exact: true }).hover();
  const grip = await page.getByRole('button', { name: 'Move x' }).boundingBox();
  if (!grip) throw new Error('grip not on screen');
  await page.mouse.move(grip.x + 5, grip.y + 5);
  await page.mouse.down();
  await page.mouse.move(grip.x + 15, grip.y + 5, { steps: 3 });
  await expect(page.locator('[data-card].z-20')).toHaveCount(1);
  expect(await addresses()).toEqual(before);
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await expect.poll(addresses).toEqual(before);
});

test('moving one of two same-named hosts moves just that card', async ({ page }) => {
  const twin = HOSTS[1];
  await boot(page, [
    { ...twin, name: 'dup', hostname: 'dup-a.example.com', tags: ['web'] },
    { ...twin, name: 'x', hostname: 'x.example.com', tags: ['web'] },
    { ...twin, name: 'dup', hostname: 'dup-b.example.com', tags: ['web'] },
    { ...twin, name: 'y', hostname: 'y.example.com' },
    { ...twin, name: 'dup', hostname: 'dup-c.example.com' }
  ]);
  const addresses = () =>
    page
      .getByText(/^root@.*:22$/)
      .evaluateAll((els) => els.map((el) => el.textContent?.trim().replace(/^root@|\.example\.com:22$/g, '')));
  const card = (hostname: string) => page.locator('[data-card]', { hasText: hostname });

  // Drag dup-b onto the left half of dup-a.
  await card('dup-b.example.com').hover();
  const grip = await card('dup-b.example.com').getByRole('button', { name: 'Move dup' }).boundingBox();
  const target = await card('dup-a.example.com').boundingBox();
  if (!grip || !target) throw new Error('card not on screen');
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
  await page.mouse.down();
  await page.mouse.move(target.x + 2, target.y + target.height / 2, { steps: 12 });
  await page.mouse.up();
  await expect.poll(addresses).toEqual(['dup-b', 'dup-a', 'x', 'y', 'dup-c']);

  // Grouped, moving the untagged dup leaves its namesakes under web where they are.
  await page.keyboard.press('g');
  await card('dup-c.example.com').getByRole('button', { name: 'Move dup' }).focus();
  await page.keyboard.press('Home');
  await expect.poll(addresses).toEqual(['dup-b', 'dup-a', 'x', 'dup-c', 'y']);

  await page.reload();
  await expect.poll(addresses).toEqual(['dup-b', 'dup-a', 'x', 'dup-c', 'y']);
});

test('rejects a new host whose name already exists', async ({ page }) => {
  await boot(page);

  await page.getByRole('button', { name: 'Add host' }).click();
  const editor = page.getByRole('dialog', { name: 'Add host' });
  await editor.getByLabel('Name', { exact: true }).fill('web-1');
  await editor.getByLabel('Hostname / IP').fill('dupe.example.com');
  await editor.getByRole('button', { name: 'Add host' }).click();

  // The editor stays open with an inline error rather than clobbering the existing host.
  await expect(editor).toBeVisible();
  await expect(editor.getByText('A host named "web-1" already exists')).toBeVisible();
});
