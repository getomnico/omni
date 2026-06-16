<script lang="ts">
    import { env } from '$env/dynamic/public'

    const version = env.PUBLIC_OMNI_VERSION || 'dev'
    const commitHash = env.PUBLIC_OMNI_COMMIT || 'unknown'
</script>

<div
    class="border-border/70 bg-background/80 text-muted-foreground fixed right-2 bottom-2 z-[60] max-w-[min(80vw,22rem)] rounded-md border px-2 py-1 text-[10px] leading-tight shadow-sm backdrop-blur-sm select-text sm:right-3 sm:bottom-3"
    aria-label="Omni deployment version">
    <div>Omni {version}</div>
    <div class="break-all" title={commitHash}>Commit {commitHash}</div>
</div>
