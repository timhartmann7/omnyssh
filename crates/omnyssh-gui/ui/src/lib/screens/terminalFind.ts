// The terminal's find bar (TerminalFind.svelte) drives xterm's search addon; what it
// reads back is a match index and count, turned into the bar's label here so the
// wording can be tested without a terminal.

/** Matches the addon highlights before it stops counting; past it the count is "N+". */
export const HIGHLIGHT_LIMIT = 1000;

export type FindResult =
  | { kind: 'idle' }
  | { kind: 'invalid' }
  | { kind: 'matches'; index: number; count: number };

/** The label beside the query: empty until something is typed. The addon reports
 *  index -1 when no match is selected, which happens once the count hits the limit. */
export function findLabel(result: FindResult): string {
  if (result.kind === 'idle') return '';
  if (result.kind === 'invalid') return 'Invalid pattern';
  const { index, count } = result;
  if (count === 0) return 'No results';
  const total = count >= HIGHLIGHT_LIMIT ? `${HIGHLIGHT_LIMIT}+` : String(count);
  return index < 0 ? `${total} matches` : `${index + 1} of ${total}`;
}

/** What the bar starts with when it opens over a selection: the selected text, if it
 *  is on one line. A multi-line selection would never match a single line. */
export function seedQuery(selection: string): string {
  return selection.includes('\n') || selection.includes('\r') ? '' : selection;
}
