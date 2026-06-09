<script lang="ts">
    /**
     * Renders the human-readable location subtitle for an IMAP email citation.
     *
     * The source string is emitted by the IMAP connector as a `doc_source`
     * document attribute and forwarded to the chat UI as the Anthropic
     * SearchResultBlockParam `source` field.
     *
     * Format: imap:{account} / {folder} / {YYYY-MM-DD} / {subject}
     * Returns nothing when the source does not match the imap: prefix.
     */
    let { source }: { source: string | undefined } = $props()

    /**
     * Parse the IMAP source label into its constituent parts.
     * Returns null when the source does not start with "imap:".
     */
    function parse(
        src: string | undefined,
    ): { account: string; folder: string; date: string; subject: string } | null {
        if (!src?.startsWith('imap:')) return null
        const payload = src.slice('imap:'.length)
        const parts = payload.split(' / ')
        return {
            account: parts[0] ?? '',
            folder: parts[1] ?? '',
            date: parts[2] ?? '',
            subject: parts[3] ?? '',
        }
    }

    let meta = $derived(parse(source))
</script>

{#if meta && (meta.account || meta.folder || meta.date || meta.subject)}
    <div class="mt-0.5">
        {#if meta.subject}
            <p class="text-foreground/80 truncate text-xs font-medium">{meta.subject}</p>
        {/if}
        <p class="text-muted-foreground/70 text-xs">
            {#if meta.account}{meta.account}{/if}{#if meta.account && meta.folder} · {/if}{#if meta.folder}{meta.folder}{/if}{#if (meta.account || meta.folder) && meta.date} · {/if}{#if meta.date}{meta.date}{/if}
        </p>
    </div>
{/if}
