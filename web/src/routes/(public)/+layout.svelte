<script lang="ts">
    import '../../app.css'
</script>

<div class="from-background to-muted min-h-screen bg-gradient-to-br">
    <div class="flex min-h-screen items-center justify-center p-4">
        <div class="w-full max-w-md">
            <div class="mb-8 text-center">
                <h1 class="text-foreground text-3xl font-bold">Clio</h1>
                <p class="text-muted-foreground mt-2">Enterprise Search Platform</p>
            </div>
            <slot />
        </div>
    </div>
</div>
