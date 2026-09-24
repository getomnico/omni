#!/usr/bin/env node
import { mkdir, mkdtemp, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, isAbsolute, join, normalize, relative, resolve, sep } from 'node:path'
import { tmpdir } from 'node:os'
import { build } from 'vite'

const SDK_ROOT = '/opt/omni/component-sdk'
const MAX_SOURCE_BYTES = 1_000_000
const MAX_OUTPUT_BYTES = 10_000_000

function fail(message) {
  throw new Error(message)
}

function parseArgs(argv) {
  const args = new Map()
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index]
    if (!key.startsWith('--') || index + 1 >= argv.length) fail('Usage: omni-component-build --source <path> --output <path>')
    args.set(key.slice(2), argv[index + 1])
    index += 1
  }
  const source = args.get('source')
  const output = args.get('output')
  if (!source || !output || args.size !== 2) fail('Usage: omni-component-build --source <path> --output <path>')
  return { source, output }
}

function validateRelativePath(path, extension) {
  if (typeof path !== 'string' || path.length === 0 || path.length > 256 || isAbsolute(path)) {
    fail('Component paths must be non-empty relative paths of at most 256 characters')
  }
  if (path.includes('\\') || path.split('/').some((part) => part === '' || part === '.' || part === '..')) {
    fail('Component paths may not contain empty, dot, or parent path segments')
  }
  if (!path.endsWith(extension)) fail(`Component path must end in ${extension}`)
}

function assertInside(root, candidate) {
  const rel = relative(root, candidate)
  if (rel === '' || rel.startsWith(`..${sep}`) || isAbsolute(rel)) fail('Component path escapes the chat workspace')
}

async function main() {
  const { source, output } = parseArgs(process.argv.slice(2))
  validateRelativePath(source, '.svelte')
  validateRelativePath(output, '.html')
  if (source === output) fail('Component source and output must be different files')

  const workspace = resolve(process.cwd())
  const sourcePath = resolve(workspace, normalize(source))
  const outputPath = resolve(workspace, normalize(output))
  assertInside(workspace, sourcePath)
  assertInside(workspace, outputPath)

  const sourceInfo = await stat(sourcePath).catch(() => null)
  if (!sourceInfo?.isFile()) fail(`Component source not found: ${source}`)
  if (sourceInfo.size > MAX_SOURCE_BYTES) fail(`Component source exceeds ${MAX_SOURCE_BYTES} bytes`)

  const buildDir = await mkdtemp(join(tmpdir(), 'omni-component-'))
  const outputDir = join(buildDir, 'dist')
  try {
    const entryScript = join(buildDir, 'main.js')
    const entryCss = join(buildDir, 'omni.css')
    const entryHtml = join(buildDir, 'index.html')
    const sourceImport = JSON.stringify(sourcePath)
    await writeFile(
      entryScript,
      `import { mount } from 'svelte'\nimport App from ${sourceImport}\nimport './omni.css'\n\nconst target = document.getElementById('app')\nif (!target) throw new Error('Omni component mount target is missing')\nmount(App, { target })\n`,
      'utf8',
    )
    await writeFile(
      entryCss,
      `@source ${JSON.stringify(workspace)};\n@import ${JSON.stringify(`${SDK_ROOT}/src/theme.css`)};\n`,
      'utf8',
    )
    await writeFile(
      entryHtml,
      '<!doctype html><html><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1"></head><body><div id="app"></div><script type="module" src="/main.js"></script></body></html>',
      'utf8',
    )

    process.env.OMNI_COMPONENT_WORKSPACE = workspace
    process.env.OMNI_COMPONENT_BUILD_ROOT = buildDir
    await build({
      configFile: join(SDK_ROOT, 'vite.config.mjs'),
      root: buildDir,
      base: './',
      build: {
        outDir: outputDir,
        rollupOptions: { input: entryHtml },
      },
    })

    const htmlPath = join(outputDir, 'index.html')
    const html = await readFile(htmlPath, 'utf8')
    if (Buffer.byteLength(html, 'utf8') > MAX_OUTPUT_BYTES) fail(`Built component exceeds ${MAX_OUTPUT_BYTES} bytes`)
    if (/<(?:script|link)\b[^>]+(?:src|href)=/i.test(html)) fail('Built component contains an external script or stylesheet')
    if (/<(?:img|video|audio|source)\b[^>]+src=["'](?:https?:|\/\/)/i.test(html)) fail('Built component contains an external media URL')
    if (/(?:@import|url)\s*\(\s*["']?(?:https?:|\/\/)/i.test(html)) fail('Built component contains an external stylesheet or font URL')

    const temporaryOutput = `${outputPath}.omni-tmp-${process.pid}`
    await mkdir(dirname(outputPath), { recursive: true })
    await writeFile(temporaryOutput, html, { encoding: 'utf8', flag: 'wx' })
    await rename(temporaryOutput, outputPath)
  } finally {
    await rm(buildDir, { recursive: true, force: true })
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
  process.exitCode = 1
})
