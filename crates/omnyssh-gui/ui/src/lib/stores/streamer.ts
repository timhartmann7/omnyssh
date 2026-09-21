import { writable } from 'svelte/store';

// Streamer mode: a privacy pref that swaps real host addresses in the UI for
// deterministic fake-but-realistic ones, so IPs never appear on screen while recording.
// It is a pure display transform — the real address still drives every connection. The
// pref persists like the other UI-chrome prefs (tauri-plugin-store + a localStorage
// mirror for first paint, tech-gui.md §4.3), matching the sidebar-collapse shape.
const LOCAL_KEY = 'omnyssh-streamer-mode';
const STORE_FILE = 'settings.json';
const STORE_KEY = 'streamerMode';

function mirrored(): boolean {
  try {
    return localStorage.getItem(LOCAL_KEY) === 'true';
  } catch {
    return false; // localStorage unavailable: default to off.
  }
}

function mirrorLocal(on: boolean): void {
  try {
    localStorage.setItem(LOCAL_KEY, String(on));
  } catch {
    // localStorage unavailable (hardened webview): the store copy is canonical.
  }
}

async function persistStore(on: boolean): Promise<void> {
  try {
    const { load } = await import('@tauri-apps/plugin-store');
    const store = await load(STORE_FILE);
    await store.set(STORE_KEY, on);
    await store.save();
  } catch {
    // Not under Tauri (tests, vite preview): the localStorage mirror suffices.
  }
}

function createStreamerMode() {
  const initial = mirrored();
  const { subscribe, set: setStore } = writable<boolean>(initial);
  let current = initial;
  let interacted = false;

  function apply(on: boolean, user: boolean): void {
    current = on;
    setStore(on);
    mirrorLocal(on);
    if (user) {
      interacted = true;
      void persistStore(on);
    }
  }

  return {
    subscribe,
    set: (on: boolean) => apply(on, true),
    toggle: () => apply(!current, true),
    /** Reconcile with the canonical tauri-plugin-store value once Tauri is reachable. */
    async hydrate(): Promise<void> {
      try {
        const { load } = await import('@tauri-apps/plugin-store');
        const store = await load(STORE_FILE);
        const saved = await store.get<boolean>(STORE_KEY);
        if (!interacted && typeof saved === 'boolean') apply(saved, false);
      } catch {
        // Store unreachable: keep the mirrored value.
      }
    }
  };
}

export const streamerMode = createStreamerMode();

// --- Address masking (pure, deterministic) --------------------------------------

// A 32-bit FNV-1a-style hash (via Math.imul) so a host always maps to the same
// disguised address for the whole session — stable across cards, the palette, and
// refreshes. Salted so each octet/segment derives an independent value.
function hash(seed: string, salt: number): number {
  let h = (0x811c9dc5 ^ salt) >>> 0;
  for (let i = 0; i < seed.length; i++) {
    h ^= seed.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

// First octets that read as real public space, so the disguise never looks private/local.
const PUBLIC_FIRST_OCTET = [
  23, 45, 51, 62, 72, 84, 91, 103, 116, 128, 139, 146, 151, 167, 178, 185, 193, 201, 209, 217
];

// Pseudo-words for disguised domains; a real-looking hostname label.
const DOMAIN_WORDS = [
  'nova', 'atlas', 'harbor', 'quartz', 'vertex', 'cobalt', 'summit', 'delta', 'onyx',
  'cedar', 'orbit', 'flux', 'ridge', 'pixel', 'crimson', 'zephyr'
];

function isIpv4(host: string): boolean {
  const parts = host.split('.');
  return parts.length === 4 && parts.every((p) => /^\d{1,3}$/.test(p) && Number(p) <= 255);
}

function fakeIpv4(host: string): string {
  const first = PUBLIC_FIRST_OCTET[hash(host, 1) % PUBLIC_FIRST_OCTET.length];
  const b = hash(host, 2) % 256;
  const c = hash(host, 3) % 256;
  const d = 1 + (hash(host, 4) % 254); // 1..254 — skip .0 and .255
  return `${first}.${b}.${c}.${d}`;
}

function fakeIpv6(host: string): string {
  const g = (salt: number): string => (hash(host, salt) & 0xffff).toString(16).padStart(4, '0');
  return `2a02:${g(1)}:${g(2)}::${g(3)}`;
}

function fakeDomain(host: string): string {
  const labels = host.split('.');
  const tld = labels.length > 1 ? labels[labels.length - 1] : 'net';
  const word = DOMAIN_WORDS[hash(host, 5) % DOMAIN_WORDS.length];
  const n = hash(host, 6) % 100;
  return `${word}${n}.${tld}`;
}

/** A disguised address for `hostname`: masks IP and hostnames to 127.0.0.1 (or ::1 for IPv6). */
export function maskHostname(hostname: string): string {
  const h = hostname.trim();
  if (!h) return hostname;
  if (h.includes(':')) return '::1';
  return '127.0.0.1';
}

/** The address to render for a host: disguised when streamer mode is on, else the real one. */
export function displayHostname(hostname: string, streamerOn: boolean): string {
  return streamerOn ? maskHostname(hostname) : hostname;
}

function looksLikeHostOrIp(str: string): boolean {
  if (isIpv4(str) || str.includes(':')) return true;
  if (str.includes('.') && !str.includes(' ') && /^[a-zA-Z0-9.-]+$/.test(str)) return true;
  return false;
}

/** Masks a username for streamer/privacy mode to 'admin'. */
export function maskUser(user: string): string {
  const u = user.trim();
  if (!u) return user;
  return 'admin';
}

/** The username to render: disguised when streamer mode is on, else the real one. */
export function displayUser(user: string, streamerOn: boolean): string {
  return streamerOn ? maskUser(user) : user;
}

/** The host title/name to render: disguised when streamer mode is on and the name is an IP/hostname/domain or matches the host address. */
export function displayHostTitle(name: string, streamerOn: boolean, hostname?: string): string {
  if (!streamerOn) return name;
  const n = name.trim();
  if (!n) return name;
  if (looksLikeHostOrIp(n) || (hostname && n.toLowerCase() === hostname.trim().toLowerCase())) {
    return maskHostname(n);
  }
  return name;
}

