import { expect, test, type Page } from '@playwright/test';

// Settings + self-update (tech-gui.md §4.3). e2e runs against the static SPA with Tauri
// absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). The stub backs the
// update config in memory and returns an update from `check_update`; `update-available`
// is fired after `reload_hosts` (which the layout calls once its listeners are attached),
// mirroring the startup check. `plugin:app|version` is Tauri's `getVersion`.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: [], source: 'manual', hasKey: true, localForwards: [], tunnelAutostart: false, forwardAgent: false }
];

const UPDATE = {
  version: '2.0.0',
  url: 'https://github.com/timhartmann7/omnyssh/releases/tag/v2.0.0',
  tag: 'v2.0.0',
  canSelfUpdate: true
};

const INSTALLED = '1.0.0';

async function boot(
  page: Page,
  opts: {
    fireUpdateOnBoot: boolean;
    traySupport?: { available: boolean; minimize: boolean };
    version?: 'fails' | 'hangs';
  }
): Promise<void> {
  await page.addInitScript(
    ({ hosts, update, installed, fireUpdateOnBoot, traySupport, version }) => {
      let cbid = 0;
      const listeners: Record<string, number[]> = {};
      const state = {
        hosts: hosts.map((h) => ({ ...h })),
        updateConfig: { checkOnStartup: true, skipVersion: '' } as Record<string, unknown>
      };
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
              setTimeout(() => {
                fire('hosts-loaded', [...state.hosts]);
                if (fireUpdateOnBoot) fire('update-available', { info: update });
              }, 0);
              return Promise.resolve(null);
            case 'refresh_metrics':
              return Promise.resolve(null);
            case 'load_update_config':
              return Promise.resolve({ ...state.updateConfig });
            case 'save_update_config':
              state.updateConfig = { ...(args.config as Record<string, unknown>) };
              win.__savedUpdateConfig = { ...(args.config as Record<string, unknown>) };
              return Promise.resolve(null);
            case 'check_update':
              return Promise.resolve({ ...update });
            case 'plugin:app|version':
              if (version === 'fails') return Promise.reject('no Tauri runtime');
              if (version === 'hangs') return new Promise(() => {});
              return Promise.resolve(installed);
            case 'set_tray_behavior':
              ((win.__tray ??= []) as unknown[]).push({ ...args });
              return Promise.resolve({ ...traySupport });
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
    {
      hosts: HOSTS,
      update: UPDATE,
      installed: INSTALLED,
      fireUpdateOnBoot: opts.fireUpdateOnBoot,
      traySupport: opts.traySupport ?? { available: true, minimize: true },
      version: opts.version
    }
  );
  await page.goto('/');
  await expect(page.getByText('web-1', { exact: true })).toBeVisible();
}

test('the footer gear opens Settings; theme, interval, and update prefs work', async ({ page }) => {
  await boot(page, { fireUpdateOnBoot: false });

  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();

  // Theme mirrors the sidebar toggle (§5.1): the app boots dark; picking Light flips it.
  // `exact` avoids the sidebar toggle whose label reads "Switch to light theme".
  await page.getByRole('button', { name: 'Light', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await page.getByRole('button', { name: 'Dark', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  // Auto-refresh interval is a segmented pref.
  const tenSec = page.getByRole('button', { name: '10s', exact: true });
  await tenSec.click();
  await expect(tenSec).toHaveAttribute('aria-pressed', 'true');

  // Check-on-startup persists via save_update_config (seeded true → toggles false).
  const startupSwitch = page.getByRole('switch', { name: 'Check for updates on startup' });
  await expect(startupSwitch).toHaveAttribute('aria-checked', 'true');
  await startupSwitch.click();
  await expect(startupSwitch).toHaveAttribute('aria-checked', 'false');

  // A manual check surfaces the available version and raises the banner.
  await page.getByRole('button', { name: 'Check now' }).click();
  await expect(page.getByText('Version 2.0.0 is available.')).toBeVisible();
  await expect(page.getByText('Update available — v2.0.0')).toBeVisible();
});

test('About shows the installed version, selectable for a bug report', async ({ page }) => {
  await boot(page, { fireUpdateOnBoot: false });
  await page.getByRole('button', { name: 'Settings' }).click();

  const about = page.getByRole('heading', { name: 'About' }).locator('..');
  await expect(about.getByText('Version', { exact: true })).toBeVisible();
  const version = about.getByText(INSTALLED, { exact: true });
  await expect(version).toBeVisible();
  await expect(version).toHaveCSS('user-select', 'text');
});

for (const version of ['fails', 'hangs'] as const) {
  test(`a version read that ${version} leaves no row, and the update prefs still load`, async ({
    page
  }) => {
    await boot(page, { fireUpdateOnBoot: false, version });
    await page.getByRole('button', { name: 'Settings' }).click();

    await expect(
      page.getByRole('switch', { name: 'Check for updates on startup' })
    ).toHaveAttribute('aria-checked', 'true');
    await expect(page.getByRole('heading', { name: 'About' })).toHaveCount(0);
    await expect(page.getByText('Version', { exact: true })).toHaveCount(0);
  });
}

test('scrolled to the end, Settings keeps its bottom padding', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 480 });
  await boot(page, { fireUpdateOnBoot: false });
  await page.getByRole('button', { name: 'Settings' }).click();

  const last = page.getByRole('heading', { name: 'About' }).locator('..');
  await expect(last).toBeVisible();
  const { overflows, gap } = await last.evaluate((card) => {
    const scroller = card.closest('.overflow-auto') as HTMLElement;
    scroller.scrollTop = scroller.scrollHeight;
    return {
      overflows: scroller.scrollHeight > scroller.clientHeight,
      gap: scroller.getBoundingClientRect().bottom - card.getBoundingClientRect().bottom
    };
  });
  expect(overflows).toBe(true);
  expect(gap).toBeGreaterThanOrEqual(24);
});

test('startup update-available raises the banner; dismiss hides it', async ({ page }) => {
  await boot(page, { fireUpdateOnBoot: true });

  const banner = page.getByText('Update available — v2.0.0');
  await expect(banner).toBeVisible();

  await page.getByRole('button', { name: 'Dismiss update notice' }).click();
  await expect(banner).toHaveCount(0);
});

test('a settings toggle preserves a skipVersion the banner wrote out-of-band', async ({ page }) => {
  await boot(page, { fireUpdateOnBoot: true });

  // Open Settings first so its config cache is seeded stale (skipVersion: '').
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();

  // Now Skip on the banner: it writes skipVersion='2.0.0' to the shared config.
  await page.getByRole('button', { name: 'Skip', exact: true }).click();
  await expect(page.getByText('Update available — v2.0.0')).toHaveCount(0);

  // Flipping check-on-startup must read-modify-write fresh, not clobber the skip.
  const startupSwitch = page.getByRole('switch', { name: 'Check for updates on startup' });
  await startupSwitch.click();
  await expect(startupSwitch).toHaveAttribute('aria-checked', 'false');

  const saved = await page.evaluate(
    () => (window as unknown as { __savedUpdateConfig?: Record<string, unknown> }).__savedUpdateConfig
  );
  expect(saved?.skipVersion).toBe('2.0.0');
  expect(saved?.checkOnStartup).toBe(false);
});

const trayCalls = (page: Page) =>
  page.evaluate(() => (window as unknown as { __tray?: unknown[] }).__tray ?? []);

// The Desktop Chrome device reports a Windows user agent.
test('the tray settings reach the backend and survive a restart', async ({ page }) => {
  await boot(page, { fireUpdateOnBoot: false });
  // Off by default, and the backend is told so on start.
  await expect.poll(() => trayCalls(page)).toEqual([{ minimizeToTray: false, closeToTray: false }]);

  await page.getByRole('button', { name: 'Settings' }).click();
  const close = page.getByRole('switch', { name: 'Close to tray' });
  await expect(page.getByRole('switch', { name: 'Minimize to tray' })).toHaveAttribute(
    'aria-checked',
    'false'
  );
  await close.click();
  await expect(close).toHaveAttribute('aria-checked', 'true');
  await expect
    .poll(async () => (await trayCalls(page)).at(-1))
    .toEqual({ minimizeToTray: false, closeToTray: true });

  await page.reload();
  await expect(page.getByText('web-1', { exact: true })).toBeVisible();
  await expect.poll(() => trayCalls(page)).toEqual([{ minimizeToTray: false, closeToTray: true }]);
});

test('without a system tray the settings say so and stay off', async ({ page }) => {
  await boot(page, { fireUpdateOnBoot: false, traySupport: { available: false, minimize: false } });

  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByText('This desktop has no system tray')).toBeVisible();
  await expect(page.getByRole('switch', { name: 'Close to tray' })).toBeDisabled();
  await expect(page.getByRole('switch', { name: 'Minimize to tray' })).toBeDisabled();
});

test('under Wayland the window closes to the tray but cannot minimize into it', async ({
  page
}) => {
  await boot(page, { fireUpdateOnBoot: false, traySupport: { available: true, minimize: false } });

  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByRole('switch', { name: 'Minimize to tray' })).toBeDisabled();
  await expect(page.getByText(/Not on Wayland/)).toBeVisible();
  await expect(page.getByRole('switch', { name: 'Close to tray' })).toBeEnabled();
});

test.describe('on macOS', () => {
  test.use({
    userAgent:
      'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)'
  });

  test('the window closes to the menu bar, and minimizing stays with the Dock', async ({
    page
  }) => {
    await boot(page, { fireUpdateOnBoot: false });

    await page.getByRole('button', { name: 'Settings' }).click();
    await expect(page.getByRole('switch', { name: 'Close to the menu bar' })).toBeVisible();
    await expect(page.getByRole('switch', { name: 'Minimize to tray' })).toHaveCount(0);
  });
});
