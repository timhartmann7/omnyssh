// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

// Same persistence shape as the sidebar collapse (./ui.test.ts): a fake Tauri store
// and a fresh module per test to reset the singleton.
const backend = { get: vi.fn(), set: vi.fn(), save: vi.fn() };
vi.mock('@tauri-apps/plugin-store', () => ({ load: vi.fn(async () => backend) }));

const LOCAL_KEY = 'omnyssh-dashboard-view';

async function fresh() {
  vi.resetModules();
  return (await import('./dashboardView')).dashboardView;
}

const DEFAULTS = { groupByTag: false, collapsed: [], sort: 'custom', order: [] };

describe('dashboard view prefs', () => {
  beforeEach(() => {
    localStorage.clear();
    backend.get.mockReset();
    backend.set.mockReset().mockResolvedValue(undefined);
    backend.save.mockReset().mockResolvedValue(undefined);
  });

  it('starts ungrouped, unfolded, in the custom order', async () => {
    const view = await fresh();
    expect(get(view)).toEqual(DEFAULTS);
  });

  it('folds and unfolds sections', async () => {
    const view = await fresh();
    view.toggleCollapsed('prod');
    view.toggleCollapsed('');
    expect(get(view).collapsed).toEqual(['prod', '']);
    view.toggleCollapsed('prod');
    expect(get(view).collapsed).toEqual(['']);
  });

  it('a stored order switches the sort to custom', async () => {
    const view = await fresh();
    view.setSort('name');
    expect(get(view).sort).toBe('name');
    view.setOrder(['db', 'web']);
    expect(get(view)).toMatchObject({ sort: 'custom', order: ['db', 'web'] });
  });

  it('mirrors changes to localStorage and initialises from it', async () => {
    const view = await fresh();
    view.toggleGroupByTag();
    view.toggleCollapsed('db');
    view.setOrder(['web', 'db']);
    view.setSort('status');
    const saved = { groupByTag: true, collapsed: ['db'], sort: 'status', order: ['web', 'db'] };
    expect(JSON.parse(localStorage.getItem(LOCAL_KEY) ?? '{}')).toEqual(saved);
    const reloaded = await fresh();
    expect(get(reloaded)).toEqual(saved);
  });

  it('survives a corrupt mirror', async () => {
    localStorage.setItem(LOCAL_KEY, '{not json');
    const view = await fresh();
    expect(get(view)).toEqual(DEFAULTS);
  });

  it('writes the canonical tauri-plugin-store on a user change', async () => {
    const view = await fresh();
    view.toggleGroupByTag();
    await vi.waitFor(() => {
      expect(backend.set).toHaveBeenCalledWith('dashboardView', { ...DEFAULTS, groupByTag: true });
      expect(backend.save).toHaveBeenCalled();
    });
  });

  it('hydrate applies the stored value and refreshes the mirror', async () => {
    const saved = { groupByTag: true, collapsed: ['web'], sort: 'name', order: ['b', 'a'] };
    backend.get.mockResolvedValue(saved);
    const view = await fresh();
    await view.hydrate();
    expect(get(view)).toEqual(saved);
    expect(JSON.parse(localStorage.getItem(LOCAL_KEY) ?? '{}')).toEqual(saved);
  });

  it('hydrate does not clobber a fresh user change', async () => {
    backend.get.mockResolvedValue({ groupByTag: false, collapsed: ['web'], sort: 'name' });
    const view = await fresh();
    view.toggleGroupByTag(); // user acts before hydrate resolves
    await view.hydrate();
    expect(get(view)).toEqual({ ...DEFAULTS, groupByTag: true });
  });

  it('hydrate falls back per field for invalid values', async () => {
    backend.get.mockResolvedValue({
      groupByTag: 'yes',
      collapsed: ['db', 1, 'db', null],
      sort: 'cpu',
      order: 'web'
    });
    const view = await fresh();
    await view.hydrate();
    expect(get(view)).toEqual({ ...DEFAULTS, collapsed: ['db'] });
  });
});
