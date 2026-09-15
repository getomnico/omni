/**
 * PWA service worker registration.
 *
 * Conservative registration: the SW itself only intercepts top-level
 * navigations with a network-first strategy and a branded recovery document
 * (see service-worker.js). Updates require user consent — a toast offers a
 * reload; only then does the waiting worker activate via SKIP_WAITING.
 */

import { toast } from 'svelte-sonner'

const UPDATE_TOAST_ID = 'omni-pwa-update'

let registrationPromise: Promise<ServiceWorkerRegistration | null> | null = null

export function registerServiceWorker(): Promise<ServiceWorkerRegistration | null> {
    if (registrationPromise) return registrationPromise
    if (!import.meta.env.PROD) {
        registrationPromise = Promise.resolve(null)
        return registrationPromise
    }
    if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) {
        registrationPromise = Promise.resolve(null)
        return registrationPromise
    }

    registrationPromise = navigator.serviceWorker
        .register('/service-worker.js')
        .then((registration) => {
            wireUpdateFlow(registration)
            return registration
        })
        .catch(() => null)

    return registrationPromise
}

function wireUpdateFlow(registration: ServiceWorkerRegistration): void {
    let applyingUpdate = false

    // Reload exactly once, and only when we asked for it — an unconditional
    // controllerchange reload drops active streams and unsent input.
    navigator.serviceWorker.addEventListener('controllerchange', () => {
        if (applyingUpdate) window.location.reload()
    })

    const applyUpdate = (worker: ServiceWorker) => {
        applyingUpdate = true
        worker.postMessage({ type: 'SKIP_WAITING' })
    }

    const offerUpdate = (worker: ServiceWorker) => {
        toast('Omni has been updated', {
            id: UPDATE_TOAST_ID,
            duration: Infinity,
            action: {
                label: 'Reload',
                onClick: () => applyUpdate(worker),
            },
        })
    }

    if (registration.waiting && navigator.serviceWorker.controller) {
        offerUpdate(registration.waiting)
    }

    registration.addEventListener('updatefound', () => {
        const installing = registration.installing
        installing?.addEventListener('statechange', () => {
            if (installing.state === 'installed' && navigator.serviceWorker.controller) {
                offerUpdate(installing)
            }
        })
    })
}
