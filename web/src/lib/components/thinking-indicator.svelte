<script lang="ts">
    import { themeStore } from '$lib/themes/store.svelte'

    type Props = {
        text: string
    }

    let { text }: Props = $props()
</script>

<span data-testid="thinking-indicator" class="thinking-container flex items-center gap-1.5">
    <img
        src={themeStore.current.omniLogoLight}
        alt="Thinking"
        class="omni-logo-light thinking-logo !m-0 rounded opacity-60"
        width="20"
        height="20" />
    <img
        src={themeStore.current.omniLogoDark}
        alt="Thinking"
        class="omni-logo-dark thinking-logo !m-0 rounded opacity-60"
        width="20"
        height="20" />
    <span class="text-muted-foreground text-sm">{text}...</span>
</span>

<style>
    @keyframes shine-sweep {
        0% {
            left: -100%;
        }
        100% {
            left: 200%;
        }
    }

    .thinking-container {
        position: relative;
        overflow: hidden;
    }

    .thinking-container::after {
        content: '';
        position: absolute;
        top: 0;
        left: -100%;
        width: 50%;
        height: 100%;
        background: linear-gradient(
            120deg,
            transparent 0%,
            rgba(255, 255, 255, 0.6) 50%,
            transparent 100%
        );
        animation: shine-sweep 2s ease-in-out infinite;
        pointer-events: none;
    }

    :global(.dark) .thinking-container::after {
        background: linear-gradient(
            120deg,
            transparent 0%,
            rgba(255, 255, 255, 0.3) 50%,
            transparent 100%
        );
    }
</style>
