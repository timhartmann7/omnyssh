// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { clampToScroller, scrollStep } from './cardDrag';

// An element whose box spans `top`..`bottom` on screen.
function at(top: number, bottom: number): HTMLElement {
  const el = document.createElement('div');
  el.getBoundingClientRect = () => new DOMRect(0, top, 100, bottom - top);
  return el;
}

describe('auto-scroll while dragging a card', () => {
  const scroller = at(0, 500);
  const tall = at(-200, 900);

  it('scrolls toward an edge, faster the deeper the pointer sits in it', () => {
    expect(scrollStep(scroller, tall, 250)).toBe(0);
    expect(scrollStep(scroller, tall, 470)).toBeGreaterThan(0);
    expect(scrollStep(scroller, tall, 499)).toBeGreaterThan(scrollStep(scroller, tall, 470));
    expect(scrollStep(scroller, tall, 5)).toBeLessThan(0);
  });

  it('stops once the cards end inside the scroller, whatever the lifted card adds', () => {
    expect(scrollStep(scroller, at(-200, 480), 499)).toBe(0);
    expect(scrollStep(scroller, at(10, 900), 5)).toBe(0);
  });
});

describe('drop point while dragging a card', () => {
  it('keeps a pointer past the scroller edge on its nearest visible row', () => {
    const scroller = at(40, 500);
    expect(clampToScroller(scroller, 520)).toBe(500);
    expect(clampToScroller(scroller, 10)).toBe(40);
    expect(clampToScroller(scroller, 250)).toBe(250);
  });
});
