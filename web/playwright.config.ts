import { defineConfig } from '@playwright/test'

export default defineConfig({
    webServer: {
        command: 'npm run build && npm run preview',
        port: 4173,
        env: {
            ...process.env,
            DATABASE_HOST: process.env.DATABASE_HOST ?? 'localhost',
            DATABASE_PORT: process.env.DATABASE_PORT ?? '5432',
            DATABASE_USERNAME: process.env.DATABASE_USERNAME ?? 'omni_dev',
            DATABASE_NAME: process.env.DATABASE_NAME ?? 'omni_dev',
            DATABASE_PASSWORD: process.env.DATABASE_PASSWORD ?? 'omni_dev_password',
            REDIS_URL: process.env.REDIS_URL ?? 'redis://localhost:6379',
            SEARCHER_URL: process.env.SEARCHER_URL ?? 'http://localhost:1',
            INDEXER_URL: process.env.INDEXER_URL ?? 'http://localhost:1',
            AI_SERVICE_URL: process.env.AI_SERVICE_URL ?? 'http://localhost:1',
            CONNECTOR_MANAGER_URL: process.env.CONNECTOR_MANAGER_URL ?? 'http://localhost:1',
            APP_URL: process.env.APP_URL ?? 'http://localhost:4173',
            OMNI_TEST_CHAT_STREAM_REPLAY_PATH:
                process.env.OMNI_TEST_CHAT_STREAM_REPLAY_PATH ??
                'e2e/fixtures/branched-chat-stream.sse',
            OMNI_TEST_CHAT_STREAM_REPLAY_FIXTURE_DIR:
                process.env.OMNI_TEST_CHAT_STREAM_REPLAY_FIXTURE_DIR ?? 'e2e/fixtures',
        },
    },
    testDir: 'e2e',
})
