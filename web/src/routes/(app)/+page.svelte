<script lang="ts">
    import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card'
    import { Button } from '$lib/components/ui/button'
    import type { PageProps } from './$types'
    import { Input } from '$lib/components/ui/input'
    import { Search } from '@lucide/svelte'
    import { goto } from '$app/navigation'

    let { data }: PageProps = $props()

    let searchQuery = $state('')

    function handleSearch() {
        console.log('calling handleSearch', searchQuery)
        if (searchQuery.trim()) {
            goto(`/search?q=${encodeURIComponent(searchQuery.trim())}`)
        }
    }

    function handleKeyPress(event: KeyboardEvent) {
        if (event.key === 'Enter') {
            handleSearch()
        }
    }
</script>

<svelte:head>
    <title>Clio - Enterprise Search</title>
</svelte:head>

<div class="container mx-auto px-4">
    <!-- Centered Search Section -->
    <div class="flex min-h-[60vh] flex-col items-center justify-center">
        <div class="mb-8 text-center">
            <h1 class="text-foreground mb-4 text-4xl font-bold">Welcome to Clio</h1>
            <p class="text-muted-foreground text-lg">
                Your unified enterprise search platform. Search across your org's data.
            </p>
        </div>

        <!-- Search Box -->
        <div class="w-full max-w-2xl">
            <div class="flex items-center rounded-full border border-gray-300 bg-white shadow-lg">
                <div class="pr-3 pl-6">
                    <Search class="h-5 w-5 text-gray-400" />
                </div>
                <Input
                    type="text"
                    bind:value={searchQuery}
                    placeholder="Ask anything..."
                    class="text-md md:text-md flex-1 rounded-full border-none bg-transparent px-0 py-4 shadow-none focus:border-none focus:ring-0 focus:outline-none focus-visible:ring-0 focus-visible:ring-offset-0"
                    onkeypress={handleKeyPress}
                />
                <Button
                    class="m-2 cursor-pointer rounded-full px-6 py-2"
                    onclick={handleSearch}
                    disabled={!searchQuery.trim()}
                >
                    Go
                </Button>
            </div>
        </div>
    </div>

    <!-- Quick Stats -->
    <div class="py-8">
        <div class="grid grid-cols-1 gap-6 md:grid-cols-3">
            <Card>
                <CardHeader>
                    <CardTitle class="text-lg">Connected Sources</CardTitle>
                </CardHeader>
                <CardContent>
                    <div class="text-foreground text-2xl font-bold">
                        {data.stats.connectedSources}
                    </div>
                    <p class="text-muted-foreground text-sm">Data sources connected</p>
                </CardContent>
            </Card>

            <Card>
                <CardHeader>
                    <CardTitle class="text-lg">Indexed Documents</CardTitle>
                </CardHeader>
                <CardContent>
                    <div class="text-foreground text-2xl font-bold">
                        {data.stats.indexedDocuments}
                    </div>
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
