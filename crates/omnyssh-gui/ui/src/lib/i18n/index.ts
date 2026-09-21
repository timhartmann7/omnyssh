import { writable, derived, get } from 'svelte/store';
import { en, type TranslationKey } from './locales/en';
import { zhCN } from './locales/zh-CN';

export type Locale = 'zh-CN' | 'en';

export interface LocaleOption {
  code: Locale;
  label: string;
}

export const SUPPORTED_LOCALES: LocaleOption[] = [
  { code: 'zh-CN', label: '简体中文' },
  { code: 'en', label: 'English' }
];

const LOCAL_KEY = 'omnyssh-locale';
const STORE_FILE = 'settings.json';
const STORE_KEY = 'locale';

const dictionaries: Record<Locale, Record<TranslationKey, string>> = {
  'zh-CN': zhCN,
  'en': en
};

function detectInitialLocale(): Locale {
  try {
    const saved = localStorage.getItem(LOCAL_KEY);
    if (saved === 'zh-CN' || saved === 'en') return saved;
  } catch {
    // localStorage unavailable
  }

  // Fallback to browser/system language
  if (typeof navigator !== 'undefined' && navigator.language) {
    if (navigator.language.toLowerCase().startsWith('zh')) {
      return 'zh-CN';
    }
  }

  // Default to zh-CN in the Chinese fork
  return 'zh-CN';
}

function mirrorLocal(l: Locale): void {
  try {
    localStorage.setItem(LOCAL_KEY, l);
  } catch {
    // Ignored in hardened webviews
  }
}

async function persistStore(l: Locale): Promise<void> {
  try {
    const { load } = await import('@tauri-apps/plugin-store');
    const store = await load(STORE_FILE);
    await store.set(STORE_KEY, l);
    await store.save();
  } catch {
    // Non-Tauri environment (e.g. tests)
  }
}

export function translate(
  currentLocale: Locale,
  key: TranslationKey,
  params?: Record<string, string | number>
): string {
  const dict = dictionaries[currentLocale] || dictionaries['zh-CN'];
  let text: string = dict[key] || dictionaries['en'][key] || key;

  if (params) {
    for (const [paramKey, value] of Object.entries(params)) {
      text = text.replaceAll(`{${paramKey}}`, String(value));
    }
  }

  return text;
}

function createLocaleStore() {
  const initial = detectInitialLocale();
  const { subscribe, set: setStore } = writable<Locale>(initial);
  let current = initial;
  let interacted = false;

  function apply(nextLocale: Locale, userInitiated: boolean): void {
    current = nextLocale;
    setStore(nextLocale);
    mirrorLocal(nextLocale);
    if (userInitiated) {
      interacted = true;
      void persistStore(nextLocale);
    }
  }

  return {
    subscribe,
    set: (nextLocale: Locale) => apply(nextLocale, true),
    get: () => current,
    async hydrate(): Promise<void> {
      try {
        const { load } = await import('@tauri-apps/plugin-store');
        const store = await load(STORE_FILE);
        const saved = await store.get<string>(STORE_KEY);
        if (!interacted && (saved === 'zh-CN' || saved === 'en')) {
          apply(saved as Locale, false);
        }
      } catch {
        // Store unreachable
      }
    }
  };
}

export const locale = createLocaleStore();

/**
 * Derived reactive translation function.
 * Usage in Svelte: `{$t('dashboard.title')}` or `{$t('dashboard.no_match', { query })}`
 */
export const t = derived(
  locale,
  ($locale) => (key: TranslationKey, params?: Record<string, string | number>) =>
    translate($locale, key, params)
);

export type { TranslationKey };
