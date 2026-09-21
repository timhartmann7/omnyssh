// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { locale, t, translate, SUPPORTED_LOCALES } from './index';

describe('i18n module', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('supports zh-CN and en locales', () => {
    expect(SUPPORTED_LOCALES).toHaveLength(2);
    expect(SUPPORTED_LOCALES.map((l) => l.code)).toEqual(['zh-CN', 'en']);
  });

  it('translates basic keys in zh-CN and en', () => {
    expect(translate('zh-CN', 'dashboard.title')).toBe('仪表盘');
    expect(translate('en', 'dashboard.title')).toBe('Dashboard');
  });

  it('interpolates parameters correctly', () => {
    expect(translate('zh-CN', 'dashboard.no_match', { query: 'test-server' })).toBe(
      '没有匹配 “test-server” 的主机。'
    );
    expect(translate('en', 'dashboard.no_match', { query: 'test-server' })).toBe(
      'No hosts match “test-server”.'
    );
  });

  it('reactive t store reflects locale changes', () => {
    locale.set('zh-CN');
    expect(get(locale)).toBe('zh-CN');
    const translateFnZh = get(t);
    expect(translateFnZh('sidebar.dashboard')).toBe('仪表盘');

    locale.set('en');
    expect(get(locale)).toBe('en');
    const translateFnEn = get(t);
    expect(translateFnEn('sidebar.dashboard')).toBe('Dashboard');
  });
});
