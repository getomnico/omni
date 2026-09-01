import { defineConfig } from '@playwright/test'

const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH

export default defineConfig({
    workers: 1,
    testDir: 'e2e',
    testMatch: 'streaming-markdown-renderer.spec.ts',
    ...(executablePath ? { use: { launchOptions: { executablePath } } } : {}),
})
