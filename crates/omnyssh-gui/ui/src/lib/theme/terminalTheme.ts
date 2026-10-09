// xterm renders outside the DOM/CSS-variable system, so its colours can't be CSS
// tokens — they're derived here from the brandbook's ink/paper language (§5.1) and
// pushed to every live terminal on theme toggle. Background/foreground/cursor mirror
// the `--surface`/`--text`/`--accent` tokens; the 16 ANSI colours are the terminal's
// content palette (what remote programs emit), reusing the app's server-state colours
// for red/green/yellow and cool blue/violet accents to stay on the monochrome brand
// tone. Both maps are tuned for AA-legible reading on their surface.
import type { ITheme } from '@xterm/xterm';
import type { Theme } from '$lib/stores/theme';

const dark: ITheme = {
  background: '#212121', // --surface (dark)
  foreground: '#e4e7ec', // paper, softened for sustained reading
  cursor: '#ffffff', // --accent (dark)
  cursorAccent: '#212121',
  selectionBackground: 'rgba(255, 255, 255, 0.18)',
  black: '#3a3f45',
  red: '#f07777', // --status-crit (dark)
  green: '#5fd08a', // --status-ok (dark)
  yellow: '#e8c15a', // --status-warn (dark)
  blue: '#7aa2f7',
  magenta: '#bb9af7',
  cyan: '#7dcfff',
  white: '#c8ccd4',
  brightBlack: '#5b6066', // graphite
  brightRed: '#ff8f8f',
  brightGreen: '#7fe0a3',
  brightYellow: '#f0d178',
  brightBlue: '#9ab8ff',
  brightMagenta: '#d0b8ff',
  brightCyan: '#9ee0ff',
  brightWhite: '#ffffff'
};

const light: ITheme = {
  background: '#ffffff', // --surface (light)
  foreground: '#212121', // ink
  cursor: '#212121', // --accent (light)
  cursorAccent: '#ffffff',
  selectionBackground: 'rgba(33, 33, 33, 0.15)',
  black: '#212121',
  red: '#c0392b', // --status-crit (light)
  green: '#1e824c', // --status-ok (light)
  yellow: '#b7791f', // --status-warn (light)
  blue: '#2f6feb',
  magenta: '#8250df',
  cyan: '#1b7c83',
  white: '#aeb7c2', // mist
  brightBlack: '#5b6066', // graphite
  brightRed: '#d64545',
  brightGreen: '#2a9d5c',
  brightYellow: '#c88a2a',
  brightBlue: '#4785ff',
  brightMagenta: '#9a5cf0',
  brightCyan: '#2596a0',
  brightWhite: '#212121'
};

const THEMES: Record<Theme, ITheme> = { dark, light };

/** The xterm theme object for the app theme; applied to every open terminal on toggle. */
export function xtermTheme(theme: Theme): ITheme {
  return THEMES[theme];
}

/** Find-bar match highlights: the --status-warn yellow, muted for every match and full
 *  for the current one, so a match never reads as a selection. The search addon takes
 *  backgrounds only as #RRGGBB; the foreground keeps its own colour on top of them. */
export interface SearchColours {
  matchBackground: string;
  matchOverviewRuler: string;
  activeMatchBackground: string;
  activeMatchBorder: string;
  activeMatchColorOverviewRuler: string;
}

const SEARCH: Record<Theme, SearchColours> = {
  dark: {
    matchBackground: '#5a4a1e',
    matchOverviewRuler: '#e8c15a',
    activeMatchBackground: '#94701c',
    activeMatchBorder: '#f0d178',
    activeMatchColorOverviewRuler: '#f0d178'
  },
  light: {
    matchBackground: '#fbe7a8',
    matchOverviewRuler: '#b7791f',
    activeMatchBackground: '#f2b84b',
    activeMatchBorder: '#b7791f',
    activeMatchColorOverviewRuler: '#b7791f'
  }
};

export function searchColours(theme: Theme): SearchColours {
  return SEARCH[theme];
}
