// Ambient types for mammoth's standalone browser bundle
// (node_modules/mammoth/mammoth.browser.min.js). The package ships no
// declarations for this entry; the public API mirrors lib/index.d.ts.
declare module 'mammoth/mammoth.browser.min.js' {
    export type MammothInput = { arrayBuffer: ArrayBuffer }
    export type MammothResult = { value: string; messages: unknown[] }

    export function convertToHtml(input: MammothInput): Promise<MammothResult>

    // UMD bundle assigns `module.exports = factory()`, so bundlers may surface
    // the API as the default export instead of named exports.
    const mammoth: {
        convertToHtml(input: MammothInput): Promise<MammothResult>
    }
    export default mammoth
}
