import { expect, test, type Page } from '@playwright/test';

// Terminal streaming vertical (tech-gui.md §3.1). e2e runs against the static SPA with
// Tauri absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). The stub is
// channel-aware: `terminal_open` captures the per-session output Channel and streams a
// prompt through it (proving raw output renders); `terminal_write` echoes a canned line
// on Enter (proving input round-trips). The host-first path (a Dashboard card's `sh`,
// no picker) is the load-bearing flow the stage requires.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: ['prod'], source: 'manual', hasKey: true, localForwards: [], tunnelAutostart: false, forwardAgent: false },
  { name: 'db-1', hostname: 'db-1.example.com', user: 'root', port: 22, tags: [], source: 'manual', hasKey: false, localForwards: [], tunnelAutostart: false, forwardAgent: false }
];

async function boot(page: Page): Promise<void> {
  await page.addInitScript(
    ({ hosts }) => {
      let cbid = 0;
      const win = window as unknown as Record<string, unknown>;
      const listeners: Record<string, number[]> = {};
      // Per-channel outgoing index — the real Channel enforces message ordering.
      const chIndex: Record<number, number> = {};
      // Backend session id -> its output channel id, so terminal_write can echo.
      const sessionChannel: Record<number, number> = {};
      let nextSession = 0;

      function sendToChannel(chId: number, text: string): void {
        const cb = win[`__cb${chId}`] as ((m: unknown) => void) | undefined;
        const index = chIndex[chId] ?? 0;
        chIndex[chId] = index + 1;
        // The raw path delivers an ArrayBuffer; mirror that so xterm's Uint8Array wrap works.
        cb?.({ message: new TextEncoder().encode(text).buffer, index });
      }

      function fireEvent(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          const cb = win[`__cb${id}`] as ((e: unknown) => void) | undefined;
          cb?.({ event, id, payload });
        }
      }
      // Lets a test simulate the remote shell exiting for a given backend session id.
      win.__fireTerminalExited = (sessionId: number, hadOutput: boolean) =>
        fireEvent('terminal-exited', { sessionId, hadOutput });
      // ...and output the backend streams to it (the end line, a late chunk); `false`
      // while the session is not open yet.
      win.__sendOutput = (sessionId: number, text: string) => {
        const chId = sessionChannel[sessionId];
        if (chId == null) return false;
        sendToChannel(chId, text);
        return true;
      };

      (win as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve(hosts);
            case 'reload_hosts':
              return Promise.resolve(null);
            case 'terminal_open': {
              const chId = (args.onOutput as { id: number }).id;
              const sid = ++nextSession;
              sessionChannel[sid] = chId;
              const open = (): number => {
                // A shell prompt proves the streamed output renders + flips status to connected.
                if (!win.__silent) setTimeout(() => sendToChannel(chId, 'omnyssh-ready> '), 0);
                return sid;
              };
              // A test can hold the open, as a slow connect does, until it calls __release.
              if (win.__holdOpen) return new Promise((resolve) => (win.__release = () => resolve(open())));
              return Promise.resolve(open());
            }
            case 'terminal_write': {
              const { sessionId, data } = args as { sessionId: number; data: number[] };
              // Every byte the shell would get, for tests that assert what a key sent.
              ((win.__writes ??= []) as number[][]).push(data);
              const chId = sessionChannel[sessionId];
              // Echo a canned result once Enter (\r == 13) arrives, so output is assertable.
              if (chId != null && data.includes(13)) {
                setTimeout(() => sendToChannel(chId, '\r\nRESULT-OK\r\n'), 0);
              }
              return Promise.resolve(null);
            }
            case 'terminal_paste':
              win.__pasted = ((win.__pasted as number | undefined) ?? 0) + 1;
              return Promise.resolve(null);
            case 'terminal_resize':
              win.__resizes = ((win.__resizes as number | undefined) ?? 0) + 1;
              return Promise.resolve(null);
            case 'terminal_close':
              return Promise.resolve(null);
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
    { hosts: HOSTS }
  );

  await page.goto('/');
  // The status-bar total confirms the app booted and `list_hosts` resolved.
  await expect(page.getByText('2 hosts')).toBeVisible();
}

test('host-first: spawn a terminal from a card, run a command, see output, then close', async ({
  page
}) => {
  await boot(page);

  // Host-first spawn — a Dashboard card's `sh`, no picker (tech-gui.md §2, §3.1).
  await page.getByTitle('sh on web-1').click();

  // The tab row appears and the terminal renders the streamed prompt.
  await expect(page.getByRole('button', { name: 'web-1 · terminal', exact: true })).toBeVisible();
  await expect(page.locator('.xterm')).toBeVisible();
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');

  // Run a command: focus the terminal input, type, press Enter -> canned output streams back.
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.type('hi');
  await page.keyboard.press('Enter');
  await expect(page.locator('.xterm-rows')).toContainText('RESULT-OK');

  // Closing the tab tears the terminal down.
  await page.getByRole('button', { name: 'Close web-1' }).click();
  await expect(page.locator('.xterm')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'web-1 · terminal', exact: true })).toHaveCount(0);
});

test('action-first: the Terminal spawner opens the host picker, then a live terminal', async ({
  page
}) => {
  await boot(page);

  // Action-first spawn — the sidebar Terminal spawner opens the host picker (§2).
  await page.getByRole('button', { name: 'Terminal', exact: true }).click();
  await page.getByRole('dialog').getByText('web-1', { exact: true }).click();

  await expect(page.getByRole('button', { name: 'web-1 · terminal', exact: true })).toBeVisible();
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');
});

test('dismissing the host picker hands the keyboard back to the terminal', async ({ page }) => {
  await boot(page);
  await page.getByTitle('sh on web-1').click();
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');
  await expect(page.locator('.xterm-helper-textarea')).toBeFocused();

  await page.getByRole('button', { name: 'SFTP', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Pick a host' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.locator('.xterm-helper-textarea')).toBeFocused();

  // Clicking the tab that is already active takes the keyboard to it as well.
  await page.getByRole('button', { name: 'web-1 · terminal', exact: true }).click();
  await expect(page.locator('.xterm-helper-textarea')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.locator('.xterm-rows')).toContainText('RESULT-OK');
});

test('a terminal that opens under the palette leaves the keyboard to it', async ({ page }) => {
  await boot(page);
  await page.evaluate(() => ((window as unknown as Record<string, unknown>).__holdOpen = true));
  await page.getByTitle('sh on web-1').click();
  await page.getByTitle('Command palette (⌘K)').click();
  const query = page.getByRole('dialog', { name: 'Command palette' }).getByRole('textbox');
  await expect(query).toBeFocused();

  await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');
  // Two frames, so the terminal's own focus pass has run.
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));

  await page.keyboard.type('db');
  await expect(query).toBeFocused();
  await expect(query).toHaveValue('db');
  expect(await page.evaluate(() => (window as unknown as { __writes?: unknown }).__writes)).toBeUndefined();
});

test('toggling the theme re-themes a live terminal (§5.1)', async ({ page }) => {
  await boot(page);
  await page.getByTitle('sh on web-1').click();
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');

  // xterm paints its scrollable viewport inline with theme.background; find that
  // element's computed colour rather than assume a class (robust across versions).
  const paintedBg = () =>
    page.evaluate(() => {
      const root = document.querySelector('.xterm');
      const els = root ? Array.from(root.querySelectorAll<HTMLElement>('*')) : [];
      const painted = els.find((el) => el.style.backgroundColor);
      return painted ? getComputedStyle(painted).backgroundColor : '';
    });

  // App defaults to dark → the dark surface (#212121).
  await expect.poll(paintedBg).toBe('rgb(33, 33, 33)');

  // The #1 theme-regression guard: flipping the store re-themes the OPEN terminal.
  await page.getByTitle('Switch to light theme').click();
  await expect.poll(paintedBg).toBe('rgb(255, 255, 255)');
});

type ExitStub = {
  __fireTerminalExited: (id: number, hadOutput: boolean) => void;
  __sendOutput: (id: number, text: string) => boolean;
  __resizes?: number;
};

const fireExited = (page: Page, id: number, hadOutput: boolean) =>
  page.evaluate(([id, hadOutput]) => {
    (window as unknown as ExitStub).__fireTerminalExited(id, hadOutput);
  }, [id, hadOutput] as const);
const sendOutput = (page: Page, id: number, text: string) =>
  page.evaluate(
    ([id, text]) => (window as unknown as ExitStub).__sendOutput(id, text),
    [id, text] as const
  );
const resizes = (page: Page) =>
  page.evaluate(() => (window as unknown as ExitStub).__resizes ?? 0);
const terminalTab = (page: Page) =>
  page.getByRole('button', { name: 'web-1 · terminal', exact: true });

// What the backend writes under a session's last output when it ends.
const END_LINE =
  '\r\n\x1b[2m[Connection closed. Press Enter to close this tab.]\x1b[0m\x1b[?25l\x1b[?9;1000;1002;1003l';

test('a remote exit before any output (terminal-exited) tears the tab down', async ({ page }) => {
  await boot(page);
  await page.getByTitle('sh on web-1').click();
  await expect(terminalTab(page)).toBeVisible();
  await expect(page.locator('.xterm')).toBeVisible();

  // The connection failed: the backend emits terminal-exited for session id 1, no output.
  await fireExited(page, 1, false);

  await expect(terminalTab(page)).toHaveCount(0);
  await expect(page.locator('.xterm')).toHaveCount(0);
});

// A server that refuses the shell says why, then ends the session. The tab
// keeps that message on screen until the user closes it.
test('a remote exit after output keeps the tab, takes no input, and Enter closes it', async ({
  page
}) => {
  await bootWithClipboard(page);
  // The session also left mouse reporting on, as htop or tmux would when cut off.
  await sendOutput(page, 1, '\x1b[?1000h\r\nPermission denied, please try again.');
  await sendOutput(page, 1, END_LINE);
  await fireExited(page, 1, true);

  await expect(page.locator('.xterm-rows')).toContainText('Permission denied, please try again.');
  await expect(page.locator('.xterm-rows')).toContainText('Press Enter to close this tab.');
  await expect(terminalTab(page).locator('[role="img"]')).toHaveAttribute('aria-label', 'off');

  // Nothing reaches the ended session: keys, ^C, a paste, or a resize.
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.type('ls');
  await page.keyboard.press('Control+C');
  await page.locator('.xterm-helper-textarea').evaluate((el) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', 'uptime');
    el.dispatchEvent(new ClipboardEvent('paste', { clipboardData, bubbles: true, cancelable: true }));
  });
  const before = await resizes(page);
  await page.setViewportSize({ width: 900, height: 640 });
  await page.evaluate(
    () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))
  );
  expect(await resizes(page)).toBe(before);
  expect(await writes(page)).toEqual([]);

  // What the screen shows can still be copied.
  await selectPrompt(page);
  await page.keyboard.press('Control+Shift+C');
  await expect.poll(() => copied(page)).toEqual(['omnyssh-ready>']);

  // Tab keeps the keyboard on the terminal, so Enter still closes the tab.
  await page.keyboard.press('Tab');
  await expect(page.locator('.xterm-helper-textarea')).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(page.locator('.xterm-helper-textarea')).toBeFocused();

  await page.keyboard.press('Enter');
  await expect(terminalTab(page)).toHaveCount(0);
  await expect(page.locator('.xterm')).toHaveCount(0);
  expect(await writes(page)).toEqual([]);
});

// A large chunk crosses the IPC asynchronously, so the first output can land after the
// exit event: it still renders, and does not bring the tab back to life.
test('output arriving after the exit renders without reviving the tab; Esc closes it', async ({
  page
}) => {
  await page.addInitScript(() => {
    (window as unknown as { __silent: boolean }).__silent = true;
  });
  await boot(page);
  await page.getByTitle('sh on web-1').click();
  await expect(terminalTab(page)).toBeVisible();

  await fireExited(page, 1, true);
  await expect.poll(() => sendOutput(page, 1, 'Permission denied, please try again.')).toBe(true);

  await expect(page.locator('.xterm-rows')).toContainText('Permission denied, please try again.');
  await expect(terminalTab(page).locator('[role="img"]')).toHaveAttribute('aria-label', 'off');

  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.press('Escape');
  await expect(terminalTab(page)).toHaveCount(0);
  expect(await writes(page)).toEqual([]);
});

// Windows and Linux copy with Ctrl+Shift+C. The Desktop Chrome device reports a Windows
// user agent, so this is the path those platforms take; the clipboard is stubbed at the
// boundary like the IPC, which also keeps parallel runs apart.
async function bootWithClipboard(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const win = window as unknown as { __copied: string[] };
    win.__copied = [];
    navigator.clipboard.writeText = (text: string) => {
      win.__copied.push(text);
      return Promise.resolve();
    };
  });
  await boot(page);
  await page.getByTitle('sh on web-1').click();
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');
}

const copied = (page: Page) =>
  page.evaluate(() => (window as unknown as { __copied: string[] }).__copied);
const writes = (page: Page) =>
  page.evaluate(() => (window as unknown as { __writes?: number[][] }).__writes ?? []);

/** Double-clicks the first word of the first row, as a user selects it. */
async function selectPrompt(page: Page): Promise<void> {
  const row = (await page.locator('.xterm-rows > div').first().boundingBox())!;
  await page.mouse.dblclick(row.x + 20, row.y + row.height / 2);
}

test('Ctrl+Shift+C copies the selection and sends the shell nothing', async ({ page }) => {
  await bootWithClipboard(page);
  await selectPrompt(page);

  await page.keyboard.press('Control+Shift+C');
  await expect.poll(() => copied(page)).toEqual(['omnyssh-ready>']);
  expect(await writes(page)).toEqual([]);

  // Bare Ctrl+C stays the interrupt, selection or not.
  await page.keyboard.press('Control+C');
  await expect.poll(() => writes(page)).toEqual([[3]]);
  expect(await copied(page)).toEqual(['omnyssh-ready>']);

  // Ctrl+Shift+V is the webview's own paste; xterm must not turn it into ^V or a V.
  await page.keyboard.press('Control+Shift+V');
  expect(await writes(page)).toEqual([[3]]);
});

test('Ctrl+Shift+C with nothing selected copies nothing', async ({ page }) => {
  await bootWithClipboard(page);
  await page.locator('.xterm-helper-textarea').focus();

  await page.keyboard.press('Control+Shift+C');
  expect(await copied(page)).toEqual([]);
  expect(await writes(page)).toEqual([]);
});

test('Ctrl+Shift+S toggles streamer mode and sends the shell no ^S', async ({ page }) => {
  await bootWithClipboard(page);
  await page.locator('.xterm-helper-textarea').focus();

  await page.keyboard.press('Control+Shift+S');
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem('omnyssh-streamer-mode')))
    .toBe('true');
  expect(await writes(page)).toEqual([]);
});

// WebKitGTK under a Russian layout reports keyCode 0 for letter keys, which is also
// what a synthetic keydown carries unless told otherwise — so this is the key event
// xterm gets there: without the fallback, Ctrl+C would send nothing at all.
test('under a non-Latin layout Ctrl+C still interrupts and Ctrl+Shift+V still pastes', async ({
  page
}) => {
  await bootWithClipboard(page);
  const press = (code: string, shiftKey: boolean, keyCode = 0) =>
    page.locator('.xterm-helper-textarea').evaluate(
      (el, init) => {
        el.dispatchEvent(new KeyboardEvent('keydown', { ...init, ctrlKey: true, bubbles: true }));
      },
      { key: '\u0441', code, shiftKey, keyCode }
    );

  await press('KeyC', false);
  await expect.poll(() => writes(page)).toEqual([[3]]);

  // Where the webview does report the key (WebView2 under the same layout), xterm
  // sends ^C itself and the fallback stays out: one ^C, not two.
  await press('KeyC', false, 67);
  await expect.poll(() => writes(page)).toEqual([[3], [3]]);

  await press('KeyV', true);
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __pasted?: number }).__pasted))
    .toBe(1);
  expect(await writes(page)).toEqual([[3], [3]]);
});

// The find bar searches the scrollback (⌘F on macOS, Ctrl+Shift+F elsewhere). Every
// key it takes stays out of the shell: the stub records each write, so a stray ^F or a
// typed query reaching the session would show up there.
const LOG = 'error one\r\nok\r\nerror two\r\nERROR three\r\n';
const findBox = (page: Page) => page.getByRole('textbox', { name: 'Find in terminal' });
const findCount = (page: Page) => page.getByRole('search').locator('[aria-live]');

test('Ctrl+Shift+F finds in the scrollback; Enter steps, Esc hands the keyboard back', async ({
  page
}) => {
  await bootWithClipboard(page);
  await sendOutput(page, 1, `\r\n${LOG}`);
  await expect(page.locator('.xterm-rows')).toContainText('ERROR three');
  await page.locator('.xterm-helper-textarea').focus();

  await page.keyboard.press('Control+Shift+F');
  await expect(findBox(page)).toBeFocused();
  await page.keyboard.type('error');
  // Case-insensitive by default: all three lines match.
  await expect(findCount(page)).toHaveText(/^\d of 3$/);
  const first = await findCount(page).textContent();
  await page.keyboard.press('Enter');
  await expect(findCount(page)).not.toHaveText(first!);
  await page.keyboard.press('Shift+Enter');
  await expect(findCount(page)).toHaveText(first!);

  await page.getByRole('button', { name: 'Match case' }).click();
  await expect(findCount(page)).toHaveText(/^\d of 2$/);
  // The toggle leaves the keyboard in the query.
  await expect(findBox(page)).toBeFocused();

  await page.keyboard.press('Escape');
  await expect(page.getByRole('search')).toHaveCount(0);
  await expect(page.locator('.xterm-helper-textarea')).toBeFocused();
  // The tab is still there, and the shell got none of it.
  await expect(terminalTab(page)).toBeVisible();
  expect(await writes(page)).toEqual([]);
});

test('find reports no results, and an invalid regex, instead of a count', async ({ page }) => {
  await bootWithClipboard(page);
  await sendOutput(page, 1, `\r\n${LOG}`);
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.press('Control+Shift+F');

  await page.keyboard.type('timeout');
  await expect(findCount(page)).toHaveText('No results');

  await findBox(page).fill('err(or');
  await page.getByRole('button', { name: 'Use regular expression' }).click();
  await expect(findCount(page)).toHaveText('Invalid pattern');

  await findBox(page).fill('^error \\w+');
  await expect(findCount(page)).toHaveText(/^\d of 3$/);
});

test('a one-line selection becomes the query', async ({ page }) => {
  await bootWithClipboard(page);
  await selectPrompt(page);
  await page.keyboard.press('Control+Shift+F');
  await expect(findBox(page)).toHaveValue('omnyssh-ready>');
  await expect(findCount(page)).toHaveText('1 of 1');

  // Pressing it again with the bar open takes the keyboard back to the query.
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.press('Control+Shift+F');
  await expect(findBox(page)).toBeFocused();
});

test('bare Ctrl+F stays with the shell', async ({ page }) => {
  await bootWithClipboard(page);
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.press('Control+F');
  await expect.poll(() => writes(page)).toEqual([[6]]);
  await expect(page.getByRole('search')).toHaveCount(0);
});

test('find still works once the session has ended', async ({ page }) => {
  await bootWithClipboard(page);
  await sendOutput(page, 1, '\r\nPermission denied, please try again.');
  await sendOutput(page, 1, END_LINE);
  await fireExited(page, 1, true);
  await expect(terminalTab(page).locator('[role="img"]')).toHaveAttribute('aria-label', 'off');

  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.press('Control+Shift+F');
  await page.keyboard.type('denied');
  await expect(findCount(page)).toHaveText('1 of 1');
  // Enter in the bar steps through matches; it does not close the ended tab.
  await page.keyboard.press('Enter');
  await expect(terminalTab(page)).toBeVisible();

  await page.keyboard.press('Escape');
  await page.keyboard.press('Enter');
  await expect(terminalTab(page)).toHaveCount(0);
});

test.describe('on macOS', () => {
  test.use({
    userAgent:
      'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)'
  });

  test('Ctrl+Shift+C is left alone — Cmd+C copies there', async ({ page }) => {
    await bootWithClipboard(page);
    await selectPrompt(page);

    await page.keyboard.press('Control+Shift+C');
    expect(await copied(page)).toEqual([]);
  });

  test('⌘F opens find, and Ctrl+Shift+F is left alone', async ({ page }) => {
    await bootWithClipboard(page);
    await page.locator('.xterm-helper-textarea').focus();

    await page.keyboard.press('Control+Shift+F');
    await expect(page.getByRole('search')).toHaveCount(0);

    await page.keyboard.press('Meta+F');
    await expect(findBox(page)).toBeFocused();
    await page.keyboard.type('ready');
    await expect(findCount(page)).toHaveText('1 of 1');
  });
});
