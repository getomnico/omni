/**
 * PWA service worker registration.
 *
 * Conservative registration: the SW itself only intercepts top-level
 * navigations with a network-first strategy and a branded recovery document
 * (see static/service-worker.js). Updates activate on the next visit via the
 * SKIP_WAITING + controllerchange reload handshake.
 */

let registrationPromise: Promise<ServiceWorkerRegistration | null> | null = null

export function registerServiceWorker(): Promise<ServiceWorkerRegistration | null> {
    if (registrationPromise) return registrationPromise
    if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) {
        registrationPromise = Promise.resolve(null)
        return registrationPromise
    }

    registrationPromise = navigator.serviceWorker
        .register('/service-worker.js')
        .then((registration) => {
            // When a new service worker is waiting, tell it to take over and
            // reload once it controls the page.
            navigator.serviceWorker.addEventListener('controllerchange', () => {
                window.location.reload()
            })
            if (registration.waiting && navigator.serviceWorker.controller) {
                registration.waiting.postMessage({ type: 'SKIP_WAITING' })
            }
            registration.addEventListener('updatefound', () => {
                const installing = registration.installing
                installing?.addEventListener('statechange', () => {
                    if (installing.state === 'installed' && navigator.serviceWorker.controller) {
                        installing.postMessage({ type: 'SKIP_WAITING' })
                    }
                })
            })
            return registration
        })
        .catch(() => null)

    return registrationPromise
}
