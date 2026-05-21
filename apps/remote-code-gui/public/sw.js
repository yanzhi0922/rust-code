const SERVICE_WORKER_VERSION =
  new URL(self.location.href).searchParams.get('v') ?? 'dev';
const CACHE_NAME = `remote-code-gui-${SERVICE_WORKER_VERSION}`;
const APP_SHELL = [
  '/index.html',
  '/manifest.webmanifest',
  '/favicon.svg',
  '/favicon.ico',
  '/apple-touch-icon.png',
  '/pwa-icon-192.png',
  '/pwa-icon-512.png',
  '/pwa-maskable-192.png',
  '/pwa-maskable-512.png',
  '/brand-mark.svg',
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(APP_SHELL)).catch(() => undefined),
  );
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys.map((key) => {
            if (key !== CACHE_NAME) {
              return caches.delete(key);
            }
            return Promise.resolve(false);
          }),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('message', (event) => {
  if (event.data?.type === 'SKIP_WAITING') {
    self.skipWaiting();
  }
});

self.addEventListener('fetch', (event) => {
  if (event.request.method !== 'GET') {
    return;
  }

  const requestUrl = new URL(event.request.url);
  if (requestUrl.origin !== self.location.origin) {
    return;
  }

  if (
    requestUrl.searchParams.has('access_token') ||
    requestUrl.searchParams.has('token') ||
    requestUrl.searchParams.has('pairing_offer') ||
    requestUrl.searchParams.has('pairing_secret') ||
    requestUrl.searchParams.has('offerId') ||
    requestUrl.searchParams.has('secret')
  ) {
    event.respondWith(
      fetch(event.request, {
        cache: 'no-store',
      }),
    );
    return;
  }

  if (requestUrl.pathname.startsWith('/v1/')) {
    event.respondWith(fetch(event.request));
    return;
  }

  if (event.request.mode === 'navigate' || event.request.destination === 'document') {
    event.respondWith(handleNavigationRequest(event.request));
    return;
  }

  if (
    event.request.destination === 'script' ||
    event.request.destination === 'style' ||
    event.request.destination === 'worker'
  ) {
    event.respondWith(handleStaticAssetRequest(event.request));
    return;
  }

  event.respondWith(handlePassiveAssetRequest(event.request));
});

async function handleNavigationRequest(request) {
  try {
    const response = await fetch(request);
    if (isCacheableResponse(response)) {
      const cache = await caches.open(CACHE_NAME);
      await cache.put('/index.html', response.clone());
    }
    return response;
  } catch {
    return (await caches.match('/index.html')) ?? Response.error();
  }
}

async function handleStaticAssetRequest(request) {
  const cached = await caches.match(request);
  if (cached) {
    return cached;
  }

  const response = await fetch(request);
  if (isCacheableResponse(response)) {
    const cache = await caches.open(CACHE_NAME);
    await cache.put(request, response.clone());
  }
  return response;
}

async function handlePassiveAssetRequest(request) {
  const cached = await caches.match(request);
  const networkPromise = fetch(request)
    .then(async (response) => {
      if (isCacheableResponse(response)) {
        const cache = await caches.open(CACHE_NAME);
        await cache.put(request, response.clone());
      }
      return response;
    })
    .catch(() => null);

  if (cached) {
    void networkPromise;
    return cached;
  }

  return (await networkPromise) ?? Response.error();
}

function isCacheableResponse(response) {
  return Boolean(response?.ok) && response.type !== 'error';
}
