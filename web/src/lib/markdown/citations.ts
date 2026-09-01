import type { TextCitationParam } from '@anthropic-ai/sdk/resources'
import { citationIdFromCitation } from '$lib/utils/citations'

export function preprocessCitationPlaceholders(
    text: string,
    citationValues?: TextCitationParam[],
): string {
    return text.replace(/\{omni-cit:([^}]+)\}/g, (match, encodedId: string) => {
        let targetId: string
        try {
            targetId = decodeURIComponent(encodedId)
        } catch {
            return match
        }

        const citationIndex =
            citationValues?.findIndex(
                (citation) => citationIdFromCitation(citation) === targetId,
            ) ?? -1
        if (citationIndex < 0) return match

        return `<span class="omni-reflink" data-citation-idx="${citationIndex}"></span>`
    })
}
