import { expect, test, type Page } from '@playwright/test';

// Snippets CRUD + execute-on-host (tech-gui.md §2.2). e2e runs against the static
// SPA; Tauri is absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). The
// stub is stateful: save/delete mutate an in-memory list that `list_snippets` reads
// back (so CRUD round-trips are observable), and `execute_snippet` delivers a
// `snippet-result` event per host through the same listener the app registers.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: ['prod'], source: 'manual', hasKey: true, localForwards: [], tunnelAutostart: false, forwardAgent: false },
  { name: 'db-1', hostname: 'db-1.example.com', user: 'root', port: 22, tags: [], source: 'manual', hasKey: false, localForwards: [], tunnelAutostart: false, forwardAgent: false }
];

const SNIPPETS = [
  { name: 'uptime', command: 'uptime -p', scope: 'global' },
  { name: 'restart-web', command: 'systemctl restart {{service}}', scope: 'global', params: ['service'] }
];

async function boot(page: Page, snippetsFixture = SNIPPETS, hostsFixture = HOSTS): Promise<void> {
  await page.addInitScript(
    ({ hosts, snippets }) => {
      let cbid = 0;
      const listeners: Record<string, number[]> = {};
      const state: { snippets: Array<Record<string, unknown>> } = { snippets: [...snippets] };
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
              return Promise.resolve(hosts);
            case 'list_snippets':
              // Return a fresh copy, as the real `list_snippets` (a freshly collected
              // Vec) does — a live-mutated reference wouldn't re-trigger the store.
              return Promise.resolve([...state.snippets]);
            case 'save_snippet': {
              const s = args.snippet as { name: string };
              const i = state.snippets.findIndex((x) => (x as { name: string }).name === s.name);
              if (i >= 0) state.snippets[i] = s;
              else state.snippets.push(s);
              return Promise.resolve(null);
            }
            case 'delete_snippet':
              state.snippets = state.snippets.filter((x) => (x as { name: string }).name !== args.name);
              return Promise.resolve(null);
            case 'execute_snippet': {
              const { snippetName, hostNames } = args as { snippetName: string; hostNames: string[] };
              setTimeout(() => {
                for (const hostName of hostNames) {
                  fire('snippet-result', { hostName, snippetName, ok: true, output: `output from ${hostName}` });
                }
              }, 0);
              return Promise.resolve(null);
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
        }
      };
    },
    { hosts: hostsFixture, snippets: snippetsFixture }
  );
  await page.goto('/');
  // The status-bar total confirms the app booted and `list_hosts` resolved.
  await expect(page.getByText(`${hostsFixture.length} hosts`)).toBeVisible();
  await page.getByRole('button', { name: 'Snippets', exact: true }).click();
  // The Snippets screen loads its list on mount.
  await expect(page.getByRole('heading', { name: 'Snippets' })).toBeVisible();
}

test('lists the saved snippets', async ({ page }) => {
  await boot(page);
  await expect(page.getByText('uptime', { exact: true })).toBeVisible();
  await expect(page.getByText('systemctl restart {{service}}')).toBeVisible();
});

test('executes a snippet on a chosen host and shows the per-host result', async ({ page }) => {
  await boot(page);

  await page.getByRole('button', { name: 'Run uptime' }).click();
  const runner = page.getByRole('dialog', { name: 'Run snippet' });
  await expect(runner).toBeVisible();

  await runner.getByRole('checkbox', { name: 'web-1' }).click();
  await runner.getByRole('button', { name: /Run on 1 host/ }).click();

  const results = page.getByRole('dialog', { name: 'Snippet results' });
  await expect(results).toBeVisible();
  await expect(results.getByText('web-1', { exact: true })).toBeVisible();
  await expect(results.getByText('output from web-1')).toBeVisible();
});

test('prompts for declared params before executing', async ({ page }) => {
  await boot(page);

  await page.getByRole('button', { name: 'Run restart-web' }).click();
  const runner = page.getByRole('dialog', { name: 'Run snippet' });
  await expect(runner).toBeVisible();

  // The declared param is prompted for.
  const param = runner.getByRole('textbox');
  await expect(param).toBeVisible();
  await param.fill('nginx');

  await runner.getByRole('checkbox', { name: 'db-1' }).click();
  await runner.getByRole('button', { name: /Run on 1 host/ }).click();

  const results = page.getByRole('dialog', { name: 'Snippet results' });
  await expect(results.getByText('output from db-1')).toBeVisible();
});

test('adds a snippet and it appears in the list', async ({ page }) => {
  await boot(page);

  await page.getByRole('button', { name: 'New snippet' }).click();
  const editor = page.getByRole('dialog', { name: 'New snippet' });
  await expect(editor).toBeVisible();

  await editor.getByLabel('Name', { exact: true }).fill('greet');
  await editor.getByLabel('Command', { exact: true }).fill('echo hi');
  await editor.getByRole('button', { name: 'Add snippet' }).click();

  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByText('greet', { exact: true })).toBeVisible();
});

test('deletes a snippet after confirmation', async ({ page }) => {
  await boot(page);
  await expect(page.getByText('uptime', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Delete uptime' }).click();
  const confirm = page.getByRole('dialog', { name: 'Delete snippet' });
  await expect(confirm).toBeVisible();
  await confirm.getByRole('button', { name: 'Delete', exact: true }).click();

  await expect(page.getByText('uptime', { exact: true })).toHaveCount(0);
});

test('rejects a new snippet whose name already exists', async ({ page }) => {
  await boot(page);

  await page.getByRole('button', { name: 'New snippet' }).click();
  const editor = page.getByRole('dialog', { name: 'New snippet' });
  await editor.getByLabel('Name', { exact: true }).fill('uptime');
  await editor.getByLabel('Command', { exact: true }).fill('whoami');
  await editor.getByRole('button', { name: 'Add snippet' }).click();

  // The editor stays open with an inline error rather than clobbering the existing one.
  await expect(editor).toBeVisible();
  await expect(editor.getByText('A snippet named "uptime" already exists')).toBeVisible();
});

test('renders duplicate-named snippets without crashing (core never dedups names)', async ({
  page
}) => {
  await boot(page, [
    { name: 'deploy', command: 'git pull', scope: 'global' },
    { name: 'deploy', command: 'systemctl restart app', scope: 'global' }
  ]);

  // A name-keyed each would throw and blank the screen; both rows must render.
  await expect(page.getByText('git pull')).toBeVisible();
  await expect(page.getByText('systemctl restart app')).toBeVisible();
});

test('renders a snippet with a repeated tag (nothing dedups tags)', async ({ page }) => {
  await boot(page, [{ name: 'backup', command: 'pg_dump app', scope: 'global', tags: ['db', 'db'] }]);

  await expect(page.getByText('pg_dump app')).toBeVisible();
  await expect(page.getByText('db', { exact: true })).toHaveCount(2);
});

test('lists a shared host name once in the runner, as the host a run uses', async ({ page }) => {
  const twin = { ...HOSTS[0], hostname: 'web-1b.example.com', user: 'admin' };
  await boot(page, SNIPPETS, [...HOSTS, twin]);

  await page.getByRole('button', { name: 'Run uptime' }).click();
  const runner = page.getByRole('dialog', { name: 'Run snippet' });
  const web = runner.getByRole('checkbox', { name: 'web-1' });
  await expect(web).toHaveCount(1);
  await expect(web).toContainText('deploy@web-1.example.com');
  await web.click();
  await expect(web).toHaveAttribute('aria-checked', 'true');
  await expect(runner.getByText('Run on 1 of 2')).toBeVisible();
});
