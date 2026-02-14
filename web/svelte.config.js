import adapter from '@sveltejs/adapter-node'
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte'

const config = {
    preprocess: vitePreprocess(),
    kit: {
        adapter: adapter(),
        csp: {
            mode: 'auto',
            directives: {
                'style-src': ['self', 'unsafe-inline'],
            },
        },
    },
}

export default config
