import { describe, expect, it } from 'vitest';
import { dropPoint } from './platform';

// Where an OS drop lands, from the position each webview reports (tauri-runtime-wry
// wraps it unscaled): it decides which pane and folder row take the drop.
describe('dropPoint', () => {
  const reported = { x: 1800, y: 600 };

  it('scales WebView2 physical pixels down on Windows', () => {
    expect(dropPoint(reported, 1.5, true, 1.5)).toEqual({ x: 1200, y: 400 });
    expect(dropPoint(reported, 1, true, 1)).toEqual(reported);
  });

  it('keeps WKWebView points on a Retina Mac and WebKitGTK ones under GDK_SCALE=2', () => {
    expect(dropPoint(reported, 2, false, 2)).toEqual(reported);
  });

  it('scales WebKitGTK logical pixels up when the page renders at 1 in a 2x window', () => {
    expect(dropPoint(reported, 2, false, 1)).toEqual({ x: 3600, y: 1200 });
  });

  it('keeps the position while the window scale is unknown', () => {
    expect(dropPoint(reported, undefined, false, 2)).toEqual(reported);
    expect(dropPoint(reported, null, false, 2)).toEqual(reported);
  });
});
