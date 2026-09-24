import { dirname, isAbsolute, resolve } from 'node:path'
import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { viteSingleFile } from 'vite-plugin-singlefile'

const sdkRoot = '/opt/omni/component-sdk'
const allowedPackages = new Set([
  '@omni/charts',
  '@omni/theme',
  '@omni/ui',
  'bits-ui',
  'clsx',
  'layerchart',
  'lucide-svelte',
  'svelte',
  'tailwind-merge',
  'tailwind-variants',
  'tw-animate-css',
])

function componentImportPolicy() {
  const workspaceRoot = process.env.OMNI_COMPONENT_WORKSPACE
  const allowedFile = (candidate) => {
    const absoluteCandidate = resolve(candidate)
    const roots = [workspaceRoot, process.env.OMNI_COMPONENT_BUILD_ROOT, sdkRoot]
      .filter(Boolean)
      .map((root) => resolve(root))
    return roots.some((root) => absoluteCandidate === root || absoluteCandidate.startsWith(`${root}/`))
  }

  return {
    name: 'omni-component-import-policy',
    enforce: 'pre',
    resolveId(source, importer) {
      if (!importer || importer.startsWith('\0') || importer.includes('/node_modules/') || importer.includes(sdkRoot)) return null
      if (source.startsWith('\0')) return null
      if (source.startsWith('.') || isAbsolute(source)) {
        const candidates = isAbsolute(source)
          ? [source, process.env.OMNI_COMPONENT_BUILD_ROOT && resolve(process.env.OMNI_COMPONENT_BUILD_ROOT, source.slice(1))]
          : [resolve(dirname(importer), source)]
        if (!candidates.some((candidate) => candidate && allowedFile(candidate))) {
          throw new Error('Component imports must stay within the chat workspace or Omni SDK')
        }
        return null
      }
      const packageName = source.startsWith('@') ? source.split('/').slice(0, 2).join('/') : source.split('/')[0]
      if (!allowedPackages.has(packageName)) {
        throw new Error(`Package '${packageName}' is not part of the Omni component SDK`)
      }
      return null
    },
  }
}

export default defineConfig({
  plugins: [componentImportPolicy(), svelte({ compilerOptions: { dev: false } }), viteSingleFile()],
  resolve: {
    alias: {
      '@omni/ui': `${sdkRoot}/src/ui/index.ts`,
      '@omni/charts': `${sdkRoot}/src/charts.ts`,
      '@omni/theme': `${sdkRoot}/src/theme.css`,
    },
  },
  build: {
    target: 'es2022',
    cssCodeSplit: false,
    assetsInlineLimit: 100_000_000,
    sourcemap: false,
    minify: 'esbuild',
    emptyOutDir: true,
  },
})
