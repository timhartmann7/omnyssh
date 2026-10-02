// The platform the webview runs on, from its user agent: WKWebView says Macintosh,
// WebView2 Windows NT, WebKitGTK X11; Linux. app.html makes the same macOS test for the
// title-bar inset, and the e2e suite emulates a platform by its user agent alone.
export const isMac = /Mac/.test(navigator.userAgent);
export const isWindows = /Windows/.test(navigator.userAgent);

/** An OS drag-and-drop position in CSS pixels. Tauri passes on what the webview
 *  reports: WebView2 counts physical pixels, WKWebView and WebKitGTK the window's
 *  logical ones, each `scale` (the window's scale factor) physical pixels wide. Those
 *  are CSS pixels while the page renders at the window's scale; WebKitGTK under
 *  GDK_SCALE=2 with GDK_DPI_SCALE=0.5 renders it at 1. */
export function dropPoint(
  position: { x: number; y: number },
  scale?: number | null,
  windows = isWindows,
  ratio = globalThis.devicePixelRatio || 1
): { x: number; y: number } {
  const k = windows ? 1 / ratio : (scale ?? ratio) / ratio;
  return { x: position.x * k, y: position.y * k };
}
