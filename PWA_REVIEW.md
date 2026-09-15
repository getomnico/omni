# Mobile PWA Review — `c9e3e0e feat(web): basic PWA harness`

Pre-PR review findings, grouped by severity. No changes made.

## High

1. **Not installable on Android** — Chrome's install criteria require both a 192px
   and a 512px icon; the manifest declares only `256x256` and `180x180`
   (`web/static/manifest.webmanifest:31-43`). The install prompt will never fire on
   Android, so the PR's core goal currently only works on iOS (via
   `apple-touch-icon`). Add real 192px and 512px icons.
2. **`icon-256.png` is actually 255×256 px** (confirmed with `file`), declared as
   `"256x256"` — the size mismatch gets the entry flagged or dropped. The same
   off-by-one exists in `favicon-dark.png` (255×256, pre-existing). Looks like a
   broken export.
3. **`purpose: "any maskable"` on a full-bleed logo** (`manifest.webmanifest:37`) —
   the icon's strokes touch the canvas edges, no safe zone. Android maskable shapes
   (circle/squircle) will visibly crop it. Split into separate `any` and padded
   `maskable` icons. The 180px Apple touch icon has the same edge bleed under iOS
   corner masking (cosmetic).

## Medium

4. **Update flow contradicts itself and force-reloads mid-session** — the SW comment
   says "leave updates waiting until the client gives the user a chance"
   (`web/static/service-worker.js:99-100`) and the client header says "updates
   activate on the next visit" (`web/src/lib/pwa/service-worker-client.ts:5-7`), but
   the client posts `SKIP_WAITING` the moment an update installs
   (`service-worker-client.ts:30-36`) and reloads on `controllerchange` (`:24-26`).
   Every deploy immediately reloads all open tabs, dropping active chat streams and
   unsent input. Either implement the documented user-consent flow (e.g. an
   "update available" toast) or fix the comments.
5. **Server errors disguised as connectivity problems** — 408/429/5xx navigations
   get the branded "check your connection" page
   (`web/static/service-worker.js:88-90,131`), replacing real error responses (this
   app has rate limiting that returns 429). Consider network-failure-only, or
   surface the actual status in the recovery copy.
6. **10s navigation timeout + abort** (`service-worker.js:8,71-86`) — slow-but-working
   loads (cold container, bad wifi) get the error page at 10s and the in-flight
   fetch is cancelled. Tune the value or drop the abort.
7. **Preload rejection skips the network fallback** — if `event.preloadResponse`
   rejects, the race at `service-worker.js:80` rejects straight to the recovery
   page without ever trying `fetch(req)`. Catch preload errors, then fall back to
   the direct fetch.

## Low

8. `/service-worker.js` is served with etag but **no `Cache-Control`** (adapter-node
   only sets `immutable` under `/_app/immutable`) → heuristic browser caching
   delays update detection up to ~24h, after which users get a surprise reload
   (compounds #4). Serve SW and manifest with `no-cache`.
9. **SW registers in dev** — no `import.meta.env.PROD` guard in
   `service-worker-client.ts`; dev-server restarts will show the branded 503 page,
   and the localhost SW persists across dev sessions.
10. `notificationclick`: if `client.focus()` rejects, `waitUntil` rejects and the
    `openWindow` fallback never runs (`service-worker.js:170-183`). Wrap `focus()`
    in try/catch. (Dormant until push is wired up.)
11. **`badge: icon-256.png`** (`service-worker.js:159`) — Android alpha-masks badge
    icons; an opaque logo becomes a solid blob. Use a monochrome glyph or drop the
    property. (Dormant.)
12. `scopedURL` passes absolute `http(s)` payload URLs through to `openWindow`
    (`service-worker.js:92-95,166`) — fine while push payloads are server-authored;
    keep it that way.
13. **Theme-color conflict** — the manifest hardcodes dark `#060a12`
    (`manifest.webmanifest:12-13`) while `app.html:16-17` ships light/dark
    media-based colors; in installed standalone the manifest wins, so light-theme
    users get dark splash/window chrome.
14. `viewport` lacks `viewport-fit=cover` (`app.html:10`) though the recovery doc
    has it (`service-worker.js:21`) — harmless today (nothing uses
    `env(safe-area-inset-*)`), but any future safe-area usage silently gets 0 until
    the meta changes.

## Verified OK

- No `src/service-worker` conflict; CSP `auto` keeps `'self'` in `script-src`
  (checked Kit 2.70 source — `strict-dynamic` is disabled), so registration is
  unaffected.
- Shortcut targets `/agents` and `/projects` exist; the Dockerfile copies
  `static/`; the first-install reload loop is correctly guarded by
  `navigator.serviceWorker.controller` checks.
- Good touches: no `user-scalable=no`, `prefers-reduced-motion` in the recovery
  document, both `*-web-app-capable` metas, and the navigation-only interception
  rationale (no app-shell caching without the hashed asset graph) is sound.

## Bottom line

Fix #1–#3 before submitting, or the "installable" claim ships broken on Android;
decide on #4's update UX deliberately.
