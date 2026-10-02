// Geometry for dragging a dashboard card within its run of cards (a grid container
// whose children are the cards). DOM-only, so Dashboard.svelte keeps just the state.

type Box = { left: number; top: number; width: number; height: number };

// How far outside the run a release still counts as a drop.
const SLACK = 24;
// Height of the band at the scroller's top and bottom edge that scrolls during a drag.
const EDGE = 48;

/** Where the card at `from` lands for a pointer at (x, y): its index among the other
 *  cards of the run, and a thin marker in the gap it drops into. Null outside the run,
 *  where a release cancels. */
export function dropTarget(run: HTMLElement, from: number, x: number, y: number): { to: number; marker: Box } | null {
  const box = run.getBoundingClientRect();
  if (x < box.left - SLACK || x > box.right + SLACK || y < box.top - SLACK || y > box.bottom + SLACK) {
    return null;
  }
  // The nearest card, in the pointer's own row first: an empty cell after a short last
  // row then means "last", not a spot in the row above.
  const rects = [...run.children].map((el) => el.getBoundingClientRect());
  let near = -1;
  let best = [Infinity, Infinity];
  rects.forEach((r, i) => {
    if (i === from) return;
    const dy = Math.max(r.top - y, 0, y - r.bottom);
    const dx = Math.max(r.left - x, 0, x - r.right);
    if (dy < best[0] || (dy === best[0] && dx < best[1])) {
      best = [dy, dx];
      near = i;
    }
  });
  if (near < 0) return null;

  // One column stacks the cards, so the drop side is above/below instead of left/right.
  const r = rects[near];
  const style = getComputedStyle(run);
  const across = style.gridTemplateColumns.split(' ').length > 1;
  const after = across ? x > r.left + r.width / 2 : y > r.top + r.height / 2;
  const to = (near > from ? near - 1 : near) + (after ? 1 : 0);
  const half = (parseFloat(across ? style.columnGap : style.rowGap) || 0) / 2;
  const marker = across
    ? { left: (after ? r.right + half : r.left - half) - 1, top: r.top, width: 2, height: r.height }
    : { left: r.left, top: (after ? r.bottom + half : r.top - half) - 1, width: r.width, height: 2 };
  return { to, marker };
}

/** The nearest scrolling ancestor, which a drag scrolls near its edges. */
export function scrollParent(el: HTMLElement): HTMLElement {
  for (let p = el.parentElement; p; p = p.parentElement) {
    if (/auto|scroll/.test(getComputedStyle(p).overflowY)) return p;
  }
  return document.documentElement;
}

// The scroller's visible band on screen.
function band(scroller: HTMLElement): { top: number; bottom: number } {
  return scroller === document.documentElement ? { top: 0, bottom: innerHeight } : scroller.getBoundingClientRect();
}

/** `y` pulled into the scroller's visible band. A pointer parked past an edge to
 *  auto-scroll (over the status bar, say) then still drops on the nearest visible row. */
export function clampToScroller(scroller: HTMLElement, y: number): number {
  const { top, bottom } = band(scroller);
  return Math.min(Math.max(y, top), bottom);
}

/** Pixels to scroll this frame with the pointer at `y`: faster the deeper it sits in
 *  the scroller's top or bottom edge band, and only while the run still extends past
 *  that edge. The lifted card itself grows the scroll area, so without that stop the
 *  scroll would chase it forever. */
export function scrollStep(scroller: HTMLElement, run: HTMLElement, y: number): number {
  const { top, bottom } = band(scroller);
  const box = run.getBoundingClientRect();
  let depth = 0;
  if (y < top + EDGE && box.top < top) depth = y - top - EDGE;
  else if (y > bottom - EDGE && box.bottom > bottom) depth = y - bottom + EDGE;
  return Math.round(Math.max(-EDGE, Math.min(EDGE, depth)) / 3);
}

/** Eats the click a browser fires on the start element when a drag ends. */
export function swallowClick(): void {
  const stop = (e: MouseEvent): void => {
    e.preventDefault();
    e.stopPropagation();
  };
  window.addEventListener('click', stop, { capture: true, once: true });
  setTimeout(() => window.removeEventListener('click', stop, { capture: true }));
}
