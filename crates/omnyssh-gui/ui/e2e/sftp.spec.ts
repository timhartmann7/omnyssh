import { expect, test, type Locator, type Page } from '@playwright/test';

// SFTP dual-pane vertical (tech-gui.md §3.2). e2e runs against the static SPA with
// Tauri absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). The stub owns
// an in-memory local + remote filesystem: `list_local_dir` returns directly, `sftp_*`
// commands fire the stamped `sftp-*` events the per-session forwarder would emit, and a
// transfer holds at a progress tick until `__completeTransfer()` fires its op-done — so
// the live progress bar is deterministically observable. Both spawn paths (a card's
// `files`, and the SFTP spawner via the host picker) are load-bearing for the stage.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: ['prod'], source: 'manual', hasKey: true, localForwards: [], tunnelAutostart: false, forwardAgent: false },
  { name: 'db-1', hostname: 'db-1.example.com', user: 'root', port: 22, tags: [], source: 'manual', hasKey: false, localForwards: [], tunnelAutostart: false, forwardAgent: false }
];

// `windows` swaps in a Windows-shaped local side: a home on C: and a second drive, D:.
// `holdProgress` keeps a transfer's first tick back until `__tick()`, as while the core
// walks a folder. `windowScale` is the window's scale factor, the page's own ratio
// unless given.
async function boot(
  page: Page,
  {
    windows = false,
    holdProgress = false,
    windowScale
  }: { windows?: boolean; holdProgress?: boolean; windowScale?: number } = {}
): Promise<void> {
  await page.addInitScript(
    ({ hosts, windows, holdProgress, windowScale }) => {
      let cbid = 0;
      const win = window as unknown as Record<string, unknown>;
      const listeners: Record<string, number[]> = {};
      let nextSession = 0;
      let nextTransfer = 0;
      // A pending transfer holds until the test fires its op-done, so the progress bar
      // is observable mid-flight (the core is sequential — one transfer at a time).
      const completions: Array<{ sessionId: number; tid: number; finish: () => void }> = [];
      // Remote listings asked for meanwhile wait behind it, in the core's one queue.
      const waiting: Array<{ sessionId: number; list: () => void }> = [];
      function busy(sessionId: number): boolean {
        return completions.some((c) => c.sessionId === sessionId);
      }
      function release(sessionId: number): void {
        if (busy(sessionId)) return;
        for (const w of waiting.filter((w) => w.sessionId === sessionId)) {
          waiting.splice(waiting.indexOf(w), 1);
          w.list();
        }
      }

      type Entry = {
        name: string;
        path: string;
        size: number;
        isDir: boolean;
        modified?: number | null;
      };
      // 2024-06-15 12:00 UTC: mid-month and midday, so the year shows in any time zone.
      const JUNE_2024 = 1718452800;
      const home = windows ? 'C:\\Users\\me' : '/home/user';
      const roots = windows ? ['C:\\', 'D:\\', 'E:\\', 'Z:\\'] : ['/'];
      const local: Record<string, Entry[]> = windows
        ? {
            [home]: [{ name: 'notes.txt', path: `${home}\\notes.txt`, size: 24, isDir: false }],
            'D:\\': [{ name: 'Media', path: 'D:\\Media', size: 0, isDir: true }]
          }
        : {
            [home]: [
              {
                name: 'notes.txt',
                path: '/home/user/notes.txt',
                size: 24,
                isDir: false,
                modified: JUNE_2024
              },
              { name: 'work', path: '/home/user/work', size: 0, isDir: true }
            ]
          };
      // Where each transfer was sent, and which sessions cancelled or closed, for the
      // test to read back.
      const downloads: string[] = [];
      const uploads: string[] = [];
      const cancels: number[] = [];
      const closes: number[] = [];
      (win as { __downloads?: string[] }).__downloads = downloads;
      (win as { __uploads?: string[] }).__uploads = uploads;
      (win as { __cancels?: number[] }).__cancels = cancels;
      (win as { __closes?: number[] }).__closes = closes;
      (win as { __fireEvent?: unknown }).__fireEvent = (event: string, payload: unknown) =>
        fireEvent(event, payload);
      const remote: Record<string, Entry[]> = {
        '/': [
          { name: 'config.yml', path: '/config.yml', size: 64, isDir: false, modified: JUNE_2024 },
          { name: 'var', path: '/var', size: 0, isDir: true }
        ],
        // A hostile server's names, each joined onto the folder as it was listed.
        '/var': [
          { name: 'a/b', path: '/var/a/b', size: 8, isDir: false },
          { name: './x', path: '/var/./x', size: 8, isDir: false },
          { name: 'ok.log', path: '/var/ok.log', size: 8, isDir: false }
        ]
      };

      function parentOf(p: string): string {
        const i = p.lastIndexOf('/');
        return i <= 0 ? '/' : p.slice(0, i);
      }
      function baseName(p: string): string {
        return p.slice(p.lastIndexOf('/') + 1);
      }
      function withParent(path: string, entries: Entry[]): Entry[] {
        if (roots.includes(path)) return entries;
        return [{ name: '..', path: parentOf(path), size: 0, isDir: true }, ...entries];
      }
      function addFile(fs: Record<string, Entry[]>, dir: string, name: string): void {
        const list = (fs[dir] ||= []);
        if (!list.some((e) => e.name === name)) {
          list.push({ name, path: dir === '/' ? `/${name}` : `${dir}/${name}`, size: 8, isDir: false });
        }
      }

      function fireEvent(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          const cb = win[`__cb${id}`] as ((e: unknown) => void) | undefined;
          cb?.({ event, id, payload });
        }
      }

      // Fire the oldest still-pending transfer's op-done (deterministic completion).
      (win as { __completeTransfer?: () => void }).__completeTransfer = () => {
        const c = completions.shift();
        c?.finish();
        if (c) release(c.sessionId);
      };
      // A progress tick for the oldest pending transfer.
      function tick(done: number, total: number): void {
        const c = completions[0];
        if (!c) return;
        fireEvent('transfer-progress', { sessionId: c.sessionId, transferId: c.tid, done, total });
      }
      (win as { __tick?: typeof tick }).__tick = tick;

      (win as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        // `getCurrentWebview()` reads these to scope OS drag-drop listeners.
        metadata: {
          currentWindow: { label: 'main' },
          currentWebview: { label: 'main', windowLabel: 'main' }
        },
        invoke: (cmd: string, args: Record<string, unknown>) => {
          if (cmd.startsWith('plugin:path')) return Promise.resolve(home);
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve(hosts);
            case 'reload_hosts':
              return Promise.resolve(null);
            case 'list_local_roots':
              return Promise.resolve(roots);
            case 'list_local_dir': {
              const path = args.path as string;
              if (path === 'E:\\') return Promise.reject({ message: 'The device is not ready. (os error 21)' });
              // A slow network drive, answering after the user has moved on.
              if (path === 'Z:\\') {
                const share = [{ name: 'Share', path: 'Z:\\Share', size: 0, isDir: true }];
                return new Promise((resolve) => setTimeout(() => resolve(share), 400));
              }
              return Promise.resolve(withParent(path, local[path] ?? []));
            }
            case 'sftp_open': {
              const sid = ++nextSession;
              setTimeout(() => fireEvent('sftp-connected', { sessionId: sid, hostName: args.hostName }), 0);
              return Promise.resolve(sid);
            }
            case 'sftp_list': {
              const { sessionId, path } = args as { sessionId: number; path: string };
              const list = () =>
                setTimeout(
                  () => fireEvent('sftp-dir-listed', { sessionId, path, entries: withParent(path, remote[path] ?? []) }),
                  0
                );
              if (busy(sessionId)) waiting.push({ sessionId, list });
              else list();
              return Promise.resolve(null);
            }
            case 'sftp_upload': {
              const { sessionId, remote: dest } = args as { sessionId: number; remote: string };
              uploads.push(dest);
              const tid = ++nextTransfer;
              if (!holdProgress) setTimeout(() => tick(4, 8), 0);
              completions.push({
                sessionId,
                tid,
                finish: () => {
                  addFile(remote, parentOf(dest), baseName(dest));
                  fireEvent('sftp-op-done', { sessionId, ok: true });
                }
              });
              return Promise.resolve(null);
            }
            case 'sftp_download': {
              const { sessionId, local: dest } = args as { sessionId: number; local: string };
              downloads.push(dest);
              const tid = ++nextTransfer;
              if (!holdProgress) setTimeout(() => tick(2, 8), 0);
              completions.push({
                sessionId,
                tid,
                finish: () => {
                  addFile(local, parentOf(dest), baseName(dest));
                  fireEvent('sftp-op-done', { sessionId, ok: true });
                }
              });
              return Promise.resolve(null);
            }
            case 'sftp_cancel': {
              // As the core does: the session's running transfer stops and says so.
              const { sessionId } = args as { sessionId: number };
              cancels.push(sessionId);
              const i = completions.findIndex((c) => c.sessionId === sessionId);
              if (i >= 0) {
                completions.splice(i, 1);
                setTimeout(() => {
                  fireEvent('sftp-op-done', { sessionId, ok: false, error: 'Transfer cancelled' });
                  release(sessionId);
                }, 0);
              }
              return Promise.resolve(null);
            }
            case 'sftp_close':
              closes.push((args as { sessionId: number }).sessionId);
              return Promise.resolve(null);
            case 'plugin:window|scale_factor':
              return Promise.resolve(windowScale ?? window.devicePixelRatio);
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
    { hosts: HOSTS, windows, holdProgress, windowScale }
  );

  await page.goto('/');
  await expect(page.getByText('2 hosts')).toBeVisible();
}

// Fire the oldest pending transfer's op-done.
async function complete(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as { __completeTransfer: () => void }).__completeTransfer());
}

test('host-first: a card’s files opens SFTP and browses both sides', async ({ page }) => {
  await boot(page);

  // Host-first spawn — a Dashboard card's `files`, no picker (tech-gui.md §2, §3.2).
  await page.getByTitle('files on web-1').click();

  await expect(page.getByRole('button', { name: 'web-1 · sftp', exact: true })).toBeVisible();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });

  // Both panes list their own filesystem, from distinct commands (local direct, remote event).
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();
});

test('round-trip: upload a local file to the remote, then download a remote file', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();

  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  // Upload: mark the local file, click Upload — the live progress bar shows mid-flight.
  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(page.getByLabel('transfer progress')).toBeVisible();

  // Complete it: op-done drains the batch, the remote pane re-lists with the new file.
  await page.evaluate(() => (window as unknown as { __completeTransfer: () => void }).__completeTransfer());
  await expect(remotePane.getByText('notes.txt')).toBeVisible();
  await expect(page.getByLabel('transfer progress')).toHaveCount(0);

  // Download: mark a remote file, click Download, complete — the local pane re-lists it.
  await remotePane.getByRole('checkbox', { name: 'Mark config.yml' }).click();
  await page.getByRole('button', { name: 'Download' }).click();
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  await page.evaluate(() => (window as unknown as { __completeTransfer: () => void }).__completeTransfer());
  await expect(localPane.getByText('config.yml')).toBeVisible();
});

test('Cancel stops the running transfer and drops the queued ones', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  // A file, then a folder queued behind it.
  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await localPane.getByRole('checkbox', { name: 'Mark work' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  const strip = page.getByLabel('transfer progress');
  await expect(strip).toBeVisible();

  await strip.getByRole('button', { name: 'Cancel transfer' }).click();
  await expect(strip).toHaveCount(0);
  await expect(page.getByText('Transfer cancelled')).toBeVisible();
  const sent = await page.evaluate(() => {
    const w = window as unknown as { __uploads: string[]; __cancels: number[] };
    return { uploads: w.__uploads, cancels: w.__cancels };
  });
  expect(sent).toEqual({ uploads: ['/notes.txt'], cancels: [1] });
});

test('closing the tab mid-transfer closes its session, which stops the transfer', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(page.getByLabel('transfer progress')).toBeVisible();

  await page.getByRole('button', { name: /^Close web-1/ }).click();
  await expect(page.getByLabel('transfer progress')).toHaveCount(0);
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __closes: number[] }).__closes))
    .toEqual([1]);
});

test('a transfer shows as preparing until its first tick, a folder of empty files too', async ({
  page
}) => {
  await boot(page, { holdProgress: true });
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('work')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  await localPane.getByRole('checkbox', { name: 'Mark work' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  const strip = page.getByLabel('transfer progress');
  await expect(strip).toContainText('Preparing work…');
  await expect(strip.getByRole('button', { name: 'Cancel transfer' })).toBeVisible();

  // Planned, and nothing in it has a byte: the core's first tick is 0 of 0.
  await page.evaluate(() => {
    (window as unknown as { __tick: (done: number, total: number) => void }).__tick(0, 0);
  });
  await expect(strip).toContainText('Uploading work');
  await expect(strip).toContainText('0 B');

  await complete(page);
  await expect(strip).toHaveCount(0);
  await expect(remotePane.getByText('work')).toBeVisible();
});

test('a remote folder opened during a transfer shows loading, then opens', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(page.getByLabel('transfer progress')).toBeVisible();

  // The listing waits behind the transfer.
  await remotePane.getByTitle('var', { exact: true }).click();
  await expect(remotePane).toHaveAttribute('aria-busy', 'true');
  await expect(remotePane.getByText('Loading…')).toBeVisible();

  // Once it is done the pane opens the folder asked for, not the one it was on.
  await complete(page);
  await expect(remotePane).toHaveAttribute('aria-busy', 'false');
  await expect(remotePane.getByTitle('/var', { exact: true })).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toHaveCount(0);
  await page.waitForTimeout(100);
  await expect(remotePane.getByTitle('/var', { exact: true })).toBeVisible();
});

test('a transfer that would replace items asks first, once for the batch', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  await complete(page);
  await expect(remotePane.getByText('notes.txt')).toBeVisible();

  // notes.txt is on the server now; the folder ticked beside it is not.
  await localPane.getByRole('checkbox', { name: 'Mark work' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  const ask = page.getByRole('dialog', { name: 'Replace existing items' });
  await expect(ask).toContainText(
    '1 item(s) already exist in /. Folders are merged and files with the same name are replaced.'
  );
  await ask.getByRole('button', { name: 'Cancel' }).click();
  await expect(ask).toHaveCount(0);
  expect(await uploads(page)).toEqual(['/notes.txt']);

  await page.getByRole('button', { name: 'Upload' }).click();
  await ask.getByRole('button', { name: 'Replace' }).click();
  await expect(ask).toHaveCount(0);
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  await complete(page);
  await expect.poll(() => uploads(page)).toEqual(['/notes.txt', '/notes.txt', '/work']);
});

test('a Replace prompt left for another tab is dropped, not brought back', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  await expect(page.getByRole('region', { name: 'web-1' }).getByText('config.yml')).toBeVisible();
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click();
  await page.getByTitle('files on db-1').click();
  await expect(page.getByRole('region', { name: 'db-1' }).getByText('config.yml')).toBeVisible();
  await page.getByRole('button', { name: 'web-1 · sftp', exact: true }).click();

  const localPane = page.getByRole('region', { name: 'Local' });
  await page.getByRole('region', { name: 'web-1' }).getByRole('checkbox', { name: 'Mark config.yml' }).click();
  await page.getByRole('button', { name: 'Download' }).click();
  await complete(page);
  await expect(localPane.getByText('config.yml')).toBeVisible();

  await page.getByRole('button', { name: 'Download' }).click();
  const ask = page.getByRole('dialog', { name: 'Replace existing items' });
  await expect(ask).toBeVisible();
  await page.keyboard.press('Control+k');
  await page.getByRole('dialog', { name: 'Command palette' }).getByRole('textbox').fill('db-1');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('region', { name: 'db-1' })).toBeVisible();
  await expect(ask).toHaveCount(0);

  await page.getByRole('button', { name: 'web-1 · sftp', exact: true }).click();
  await expect(page.getByRole('region', { name: 'web-1' })).toBeVisible();
  await expect(ask).toHaveCount(0);
  const downloads = await page.evaluate(() => (window as unknown as { __downloads: string[] }).__downloads);
  expect(downloads).toEqual(['/home/user/config.yml']);
});

test('a listed name that is not one plain name is refused, the rest downloads', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await remotePane.getByTitle('var', { exact: true }).click();
  await expect(remotePane.getByText('ok.log')).toBeVisible();

  for (const name of ['a/b', './x', 'ok.log']) {
    await remotePane.getByRole('checkbox', { name: `Mark ${name}` }).click();
  }
  await page.getByRole('button', { name: 'Download' }).click();
  await expect(page.getByText("'a/b' is not a name that can be created here.")).toBeVisible();
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  const downloads = await page.evaluate(() => (window as unknown as { __downloads: string[] }).__downloads);
  expect(downloads).toEqual(['/home/user/ok.log']);
});

test('a drop on a folder row does not ask: that folder is not on show', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  await complete(page);
  await expect(remotePane.getByText('notes.txt')).toBeVisible();

  // The local pane shows a notes.txt, but the drop goes into work/.
  await dragOnto(
    page,
    remotePane.getByTitle('notes.txt', { exact: true }),
    localPane.getByTitle('work', { exact: true })
  );
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  await expect(page.getByRole('dialog', { name: 'Replace existing items' })).toHaveCount(0);
  const downloads = await page.evaluate(() => (window as unknown as { __downloads: string[] }).__downloads);
  expect(downloads).toEqual(['/home/user/work/notes.txt']);
});

test('the panes show modification times', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  await expect(localPane.getByTitle(/^Modified .*2024/)).toBeVisible();
  await expect(remotePane.getByTitle(/^Modified .*2024/)).toBeVisible();
  await expect(page.getByText('Created')).toHaveCount(0);
});

test('a date stays on one line, and the column gives way in a narrow window', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const remotePane = page.getByRole('region', { name: 'web-1' });
  const date = remotePane.getByTitle(/^Modified /);
  await expect(date).toBeVisible();

  // As under GDK_SCALE=2 with GDK_DPI_SCALE=0.5: the rem the column is sized in shrinks,
  // the text it holds does not.
  await page.addStyleTag({
    content: 'html { font-size: 8px } [title^="Modified "] { font-size: 12px; line-height: 16px }'
  });
  const box = await date.boundingBox();
  expect(box?.height).toBeLessThan(24);

  await page.setViewportSize({ width: 900, height: 720 });
  await expect(date).toBeHidden();
  await expect(remotePane.getByText('Modified', { exact: true })).toBeHidden();
});

async function centre(locator: Locator): Promise<{ x: number; y: number }> {
  const box = await locator.boundingBox();
  if (!box) throw new Error('not laid out');
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

// Press on `from` and move past the drag threshold, holding the button.
async function startDrag(page: Page, from: Locator): Promise<void> {
  const a = await centre(from);
  await page.mouse.move(a.x, a.y);
  await page.mouse.down();
  await page.mouse.move(a.x + 20, a.y, { steps: 4 });
}

async function dragOnto(
  page: Page,
  from: Locator,
  to: Locator | { x: number; y: number }
): Promise<void> {
  await startDrag(page, from);
  // Several steps, so the drag tracks the target.
  const b = 'x' in to ? to : await centre(to);
  await page.mouse.move(b.x, b.y, { steps: 8 });
  await page.mouse.up();
}

test('drag and drop: a local file dropped on the remote pane uploads there', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  await dragOnto(page, localPane.getByTitle('notes.txt', { exact: true }), remotePane.getByText('config.yml'));
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  const uploads = await page.evaluate(() => (window as unknown as { __uploads: string[] }).__uploads);
  expect(uploads).toEqual(['/notes.txt']);
  // The drag ended away from the row it started on, so nothing was previewed.
  await expect(page.getByRole('dialog', { name: 'File preview' })).toHaveCount(0);

  await page.evaluate(() => (window as unknown as { __completeTransfer: () => void }).__completeTransfer());
  await expect(remotePane.getByText('notes.txt')).toBeVisible();
});

test('drag and drop: a remote file dropped on a local folder downloads into it', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('work')).toBeVisible();

  await dragOnto(page, remotePane.getByTitle('config.yml', { exact: true }), localPane.getByTitle('work', { exact: true }));
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  const downloads = await page.evaluate(() => (window as unknown as { __downloads: string[] }).__downloads);
  expect(downloads).toEqual(['/home/user/work/config.yml']);
});

test('drag and drop: dropping back on the same pane transfers nothing', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  await expect(localPane.getByText('work')).toBeVisible();

  await dragOnto(page, localPane.getByTitle('notes.txt', { exact: true }), localPane.getByTitle('work', { exact: true }));
  await expect(page.getByLabel('transfer progress')).toHaveCount(0);
  const uploads = await page.evaluate(() => (window as unknown as { __uploads: string[] }).__uploads);
  expect(uploads).toEqual([]);
});

test('drag and drop: Escape drops the drag, and the release after it clicks nothing', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const remotePane = page.getByRole('region', { name: 'web-1' });
  const folder = remotePane.getByTitle('var', { exact: true });
  await expect(folder).toBeVisible();

  await startDrag(page, folder);
  const named = page.getByText('var', { exact: true }).filter({ visible: true });
  await expect(named).toHaveCount(2); // the row and the ghost
  await page.keyboard.press('Escape');
  await expect(named).toHaveCount(1);

  // Back on the row it started from: no click, so the pane stays where it was.
  const back = await centre(folder);
  await page.mouse.move(back.x, back.y, { steps: 4 });
  await page.mouse.up();
  await page.waitForTimeout(100);
  await expect(remotePane.getByText('config.yml')).toBeVisible();
  const downloads = await page.evaluate(() => (window as unknown as { __downloads: string[] }).__downloads);
  expect(downloads).toEqual([]);
});

test('drag and drop: `..` dragged away and back does not go up, like any row', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  await localPane.getByTitle('work', { exact: true }).click();
  await expect(localPane.getByTitle('/home/user/work', { exact: true })).toBeVisible();

  const parent = localPane.getByTitle('..', { exact: true });
  const at = await centre(parent);
  await page.mouse.move(at.x, at.y);
  await page.mouse.down();
  await page.mouse.move(at.x, at.y + 200, { steps: 6 });
  await page.mouse.move(at.x, at.y, { steps: 6 });
  await page.mouse.up();
  await page.waitForTimeout(100);
  await expect(localPane.getByTitle('/home/user/work', { exact: true })).toBeVisible();
});

test('drag and drop: a drop on the transfer strip goes to the pane above it', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  const strip = page.getByLabel('transfer progress');
  await expect(strip).toBeVisible();

  // Released on the strip's local half: a download into the local pane's directory,
  // queued behind the upload.
  const x = (await centre(localPane)).x;
  const y = (await centre(strip)).y;
  await dragOnto(page, remotePane.getByTitle('config.yml', { exact: true }), { x, y });
  await complete(page);
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __downloads: string[] }).__downloads))
    .toEqual(['/home/user/config.yml']);
});

test('drag and drop: a remote folder dropped on the local pane downloads it whole', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('var')).toBeVisible();

  await dragOnto(page, remotePane.getByTitle('var', { exact: true }), localPane.getByTitle('notes.txt', { exact: true }));
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  const downloads = await page.evaluate(() => (window as unknown as { __downloads: string[] }).__downloads);
  expect(downloads).toEqual(['/home/user/var']);
  // The press on the folder row did not navigate into it.
  await expect(remotePane.getByText('config.yml')).toBeVisible();
});

// A drop from the OS file manager, as Tauri delivers it: the drag comes in, then drops.
async function osDrop(
  page: Page,
  paths: string[],
  position: { x: number; y: number }
): Promise<void> {
  for (const event of ['tauri://drag-enter', 'tauri://drag-drop']) {
    await page.evaluate(
      ({ event, paths, position }) => {
        const fire = (window as unknown as { __fireEvent: (e: string, p: unknown) => void }).__fireEvent;
        fire(event, { paths, position });
      },
      { event, paths, position }
    );
  }
}

function uploads(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __uploads: string[] }).__uploads);
}

// Each webview reports the drop position in its own pixels and Tauri passes it on as is:
// physical on Windows (WebView2), logical on macOS (WKWebView) and Linux (WebKitGTK).
const PLATFORMS = [
  {
    name: 'Windows',
    userAgent:
      'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0 Safari/537.36',
    physical: true
  },
  {
    name: 'macOS',
    userAgent:
      'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)',
    physical: false
  },
  {
    name: 'Linux',
    userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)',
    physical: false
  }
];

for (const platform of PLATFORMS) {
  test.describe(`on ${platform.name} at 2x`, () => {
    test.use({ userAgent: platform.userAgent, deviceScaleFactor: 2 });

    test('files and folders dropped from the OS onto the remote pane upload', async ({ page }) => {
      await boot(page);
      await page.getByTitle('files on web-1').click();
      const remotePane = page.getByRole('region', { name: 'web-1' });
      await expect(remotePane.getByText('config.yml')).toBeVisible();

      const at = await centre(remotePane.getByText('config.yml'));
      const scale = platform.physical ? 2 : 1;
      await osDrop(page, ['/tmp/photo.png', '/tmp/album/'], { x: at.x * scale, y: at.y * scale });

      // One transfer at a time: the folder goes once the file is done.
      await expect(page.getByLabel('transfer progress')).toBeVisible();
      expect(await uploads(page)).toEqual(['/photo.png']);
      await complete(page);
      await expect.poll(() => uploads(page)).toEqual(['/photo.png', '/album']);
    });
  });
}

test.describe('on Linux with the page at 1x in a 2x window', () => {
  // As WebKitGTK does under GDK_SCALE=2 with GDK_DPI_SCALE=0.5: it still reports the
  // window's logical pixels, now half a CSS pixel each.
  test.use({ userAgent: PLATFORMS[2].userAgent, deviceScaleFactor: 1 });

  test('a file dropped from the OS onto the remote pane uploads', async ({ page }) => {
    await boot(page, { windowScale: 2 });
    await page.getByTitle('files on web-1').click();
    const remotePane = page.getByRole('region', { name: 'web-1' });
    await expect(remotePane.getByText('config.yml')).toBeVisible();

    const at = await centre(remotePane.getByText('config.yml'));
    await osDrop(page, ['/tmp/photo.png'], { x: at.x / 2, y: at.y / 2 });
    await expect(page.getByLabel('transfer progress')).toBeVisible();
    expect(await uploads(page)).toEqual(['/photo.png']);
  });
});

test('drag and drop: a drive or volume dropped from the OS is refused, the rest uploads', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  const at = await centre(remotePane.getByText('config.yml'));
  await osDrop(page, ['/', 'D:\\', '/tmp/photo.png'], at);
  await expect(page.getByText('A drive or volume cannot be uploaded.')).toBeVisible();
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  expect(await uploads(page)).toEqual(['/photo.png']);
});

test('two dropped items of one name are refused, not uploaded over each other', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  const at = await centre(remotePane.getByText('config.yml'));
  await osDrop(page, ['/tmp/a/readme.txt', '/tmp/b/readme.txt'], at);
  await expect(page.getByText("Two of the items would both be 'readme.txt' in /.")).toBeVisible();
  await expect(page.getByLabel('transfer progress')).toHaveCount(0);
  expect(await uploads(page)).toEqual([]);
});

test('an OS drop reaches only the tab on show (§2 exactly-one-active)', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  await expect(page.getByRole('region', { name: 'web-1' }).getByText('config.yml')).toBeVisible();
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click();
  await page.getByTitle('files on db-1').click();
  const dbRemote = page.getByRole('region', { name: 'db-1' });
  await expect(dbRemote.getByText('config.yml')).toBeVisible();

  // The hidden web-1 tab hears the drop too, and must leave it to db-1.
  await osDrop(page, ['/tmp/photo.png'], await centre(dbRemote.getByText('config.yml')));
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  expect(await uploads(page)).toEqual(['/photo.png']);
  await page.getByRole('button', { name: 'web-1 · sftp', exact: true }).click();
  await expect(page.getByRole('region', { name: 'web-1' })).toBeVisible();
  await expect(page.getByLabel('transfer progress').filter({ visible: true })).toHaveCount(0);
});

test('an inactive tab’s modal never overlays another entity (§2 exactly-one-active)', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  await expect(page.getByRole('region', { name: 'web-1' })).toBeVisible();
  // Return to the Dashboard (a session hides it) to open a second SFTP session.
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click();
  await page.getByTitle('files on db-1').click();
  await expect(page.getByRole('region', { name: 'db-1' })).toBeVisible();

  // Open db-1's New-folder modal. The modal scrim traps the sidebar, so the ⌘K
  // navigator is the reachable way to switch entity while a modal is open.
  await page.getByRole('button', { name: 'Folder' }).click();
  await expect(page.getByRole('dialog', { name: 'New folder' })).toBeVisible();
  await page.keyboard.press('Control+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await palette.getByRole('textbox').fill('web-1');
  await page.keyboard.press('Enter');

  // Activating the web-1 session must fully hide db-1's modal — never two at once.
  await expect(page.getByRole('region', { name: 'web-1' })).toBeVisible();
  await expect(page.getByRole('dialog', { name: 'New folder' })).toHaveCount(0);
});

test('a drag lets go when its tab is hidden (§2 exactly-one-active)', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  await expect(page.getByRole('region', { name: 'web-1' }).getByText('config.yml')).toBeVisible();
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click();
  await page.getByTitle('files on db-1').click();
  const dbRemote = page.getByRole('region', { name: 'db-1' });
  await expect(dbRemote.getByText('config.yml')).toBeVisible();
  await page.getByRole('button', { name: 'web-1 · sftp', exact: true }).click();

  // A drag in web-1, and db-1 brought up with the keyboard while the button is held.
  const localPane = page.getByRole('region', { name: 'Local' });
  await startDrag(page, localPane.getByTitle('notes.txt', { exact: true }));
  const named = page.getByText('notes.txt', { exact: true }).filter({ visible: true });
  await expect(named).toHaveCount(2); // the row and the ghost
  await page.keyboard.press('Control+k');
  await page.getByRole('dialog', { name: 'Command palette' }).getByRole('textbox').fill('db-1');
  await page.keyboard.press('Enter');
  await expect(dbRemote).toBeVisible();

  // No ghost over db-1, and a release on its remote pane sends nothing.
  await expect(named).toHaveCount(1);
  const over = await centre(dbRemote.getByText('config.yml'));
  await page.mouse.move(over.x, over.y, { steps: 4 });
  await page.mouse.up();
  await page.waitForTimeout(100);
  const uploads = await page.evaluate(() => (window as unknown as { __uploads: string[] }).__uploads);
  expect(uploads).toEqual([]);
  await expect(page.getByLabel('transfer progress')).toHaveCount(0);
});

test('action-first: the SFTP spawner opens the host picker, then a live session', async ({
  page
}) => {
  await boot(page);

  await page.getByRole('button', { name: 'SFTP', exact: true }).click();
  await page.getByRole('dialog').getByText('web-1', { exact: true }).click();

  await expect(page.getByRole('button', { name: 'web-1 · sftp', exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'web-1' }).getByText('config.yml')).toBeVisible();
});

test('Windows: the local pane switches drives, and downloads land on the chosen one', async ({
  page
}) => {
  await boot(page, { windows: true });
  await page.getByTitle('files on web-1').click();

  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  const drive = localPane.getByRole('combobox', { name: 'Local drive' });
  await expect(drive).toHaveValue('C:\\');

  await drive.selectOption('D:\\');
  await expect(localPane.getByText('Media')).toBeVisible();
  await expect(drive).toHaveValue('D:\\');

  await remotePane.getByRole('checkbox', { name: 'Mark config.yml' }).click();
  await page.getByRole('button', { name: 'Download' }).click();
  await expect(page.getByLabel('transfer progress')).toBeVisible();
  const downloads = await page.evaluate(() => (window as unknown as { __downloads: string[] }).__downloads);
  expect(downloads).toEqual(['D:\\config.yml']);
});

test('Windows: a drive that fails to list says why, and the selector stays on the pane’s drive', async ({
  page
}) => {
  await boot(page, { windows: true });
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const drive = localPane.getByRole('combobox', { name: 'Local drive' });
  await expect(drive).toHaveValue('C:\\');

  await drive.selectOption('E:\\');
  await expect(localPane.getByText('The device is not ready')).toBeVisible();
  await expect(drive).toHaveValue('C:\\');

  // Another drive still opens.
  await drive.selectOption('D:\\');
  await expect(localPane.getByText('Media')).toBeVisible();
});

test('a single-root system shows no drive switch', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(localPane.getByRole('combobox', { name: 'Local drive' })).toHaveCount(0);
});

test('Windows: a slow drive opened during a download stays open once it is done', async ({
  page
}) => {
  await boot(page, { windows: true });
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await remotePane.getByRole('checkbox', { name: 'Mark config.yml' }).click();
  await page.getByRole('button', { name: 'Download' }).click();
  await expect(page.getByLabel('transfer progress')).toBeVisible();

  // Z: is still listing when the download ends.
  await localPane.getByRole('combobox', { name: 'Local drive' }).selectOption('Z:\\');
  await complete(page);
  await expect(localPane.getByText('Share')).toBeVisible();
  await expect(localPane.getByText('notes.txt')).toHaveCount(0);
});

test('Windows: a slow drive left for another does not take the pane back', async ({ page }) => {
  await boot(page, { windows: true });
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const drive = localPane.getByRole('combobox', { name: 'Local drive' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();

  await drive.selectOption('Z:\\');
  await drive.selectOption('D:\\');
  await expect(localPane.getByText('Media')).toBeVisible();
  await page.waitForTimeout(600);
  await expect(localPane.getByText('Media')).toBeVisible();
  await expect(localPane.getByText('Share')).toHaveCount(0);
  await expect(drive).toHaveValue('D:\\');
});
