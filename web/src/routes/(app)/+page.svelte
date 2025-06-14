<script lang="ts">
    import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js'
    import { Button } from '$lib/components/ui/button/index.js'
    import type { PageData } from './$types.js'
    import Input from '$lib/components/ui/input/input.svelte'

    export let data: PageData
</script>

<svelte:head>
    <title>Clio - Enterprise Search</title>
</svelte:head>

<div class="container mx-auto px-4 py-8">
    <div class="mb-8">
        <h1 class="text-foreground text-3xl font-bold">Welcome to Clio</h1>
        <p class="text-muted-foreground mt-2">
            Your unified enterprise search platform. Search across all your connected data sources.
        </p>
    </div>

    <!-- Search Box -->
    <div class="flex gap-2">
        <Input type="text" placeholder="Ask anything..." />
        <Button class="w-full sm:w-auto">Search</Button>
    </div>

    <!-- Quick Stats -->
    <div class="grid grid-cols-1 gap-6 md:grid-cols-3">
        <Card>
            <CardHeader>
                <CardTitle class="text-lg">Connected Sources</CardTitle>
            </CardHeader>
            <CardContent>
                <div class="text-foreground text-2xl font-bold">0</div>
                <p class="text-muted-foreground text-sm">Data sources connected</p>
            </CardContent>
        </Card>

        <Card>
            <CardHeader>
                <CardTitle class="text-lg">Indexed Documents</CardTitle>
            </CardHeader>
            <CardContent>
                <div class="text-foreground text-2xl font-bold">0</div>
                <p class="text-muted-foreground text-sm">Documents ready to search</p>
            </CardContent>
        </Card>

        <Card>
            <CardHeader>
                <CardTitle class="text-lg">Recent Searches</CardTitle>
            </CardHeader>
            <CardContent>
                <div class="text-foreground text-2xl font-bold">0</div>
                <p class="text-muted-foreground text-sm">Searches performed today</p>
            </CardContent>
        </Card>
    </div>

    {#if data.user.role === 'admin'}
        <div class="mt-8">
            <Card>
                <CardHeader>
                    <CardTitle>Admin Quick Actions</CardTitle>
                </CardHeader>
                <CardContent class="space-y-4">
                    <div class="flex flex-wrap gap-4">
                        <a href="/admin/users">
                            <Button variant="outline">Manage Users</Button>
                        </a>
                        <Button variant="outline">Connect Data Sources</Button>
                        <Button variant="outline">View Analytics</Button>
                    </div>
                </CardContent>
            </Card>
        </div>
    {/if}
</div>
