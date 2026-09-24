# Omni interactive components

Use this skill for an interactive chart or custom UI that should appear inline in chat.

## Build workflow

1. Create `components/App.svelte` with `write_file` (use `edit_file` for revisions).
2. Use only the fixed SDK imports below. The compiler rejects other packages; do not create Vite config, package files, or install dependencies.
3. Call `build_component(source_path="components/App.svelte", output_path="components/result.html")`.
4. Call `present_artifact(path="components/result.html", title="...", display_mode="inline", inline_height=...)`.

The output is a single offline HTML file. Do not use fetch, external URLs, remote fonts, CDN scripts, or server-side code. Browser APIs and local component state are available. Keep inline components focused; use panel mode for large documents or dashboards.

## SDK imports

```svelte
<script lang="ts">
  import { Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Input } from '@omni/ui'
  import { LineChart, AreaChart, BarChart } from '@omni/charts'
</script>
```

`@omni/ui` provides shadcn-style Button, Card, Input, Badge, Separator, and `cn`. Buttons must use the `cursor-pointer` class when adding custom classes. `@omni/charts` re-exports LayerChart primitives. Chart data is ordinary arrays; keep data local to the component.

## Chart example

```svelte
<script lang="ts">
  import { Card, CardContent, CardHeader, CardTitle } from '@omni/ui'
  import { LineChart } from '@omni/charts'

  type Point = { month: string; revenue: number }
  let points: Point[] = [
    { month: 'Jan', revenue: 120 },
    { month: 'Feb', revenue: 165 },
    { month: 'Mar', revenue: 210 },
    { month: 'Apr', revenue: 248 },
  ]
</script>

<Card class="mx-auto max-w-2xl">
  <CardHeader><CardTitle>Revenue trend</CardTitle></CardHeader>
  <CardContent>
    <div class="h-64">
      <LineChart
        data={points}
        series={[{ key: 'revenue', label: 'Revenue', value: (point) => point.revenue, color: 'var(--color-primary)' }]}
        x={(point) => point.month}
      />
    </div>
  </CardContent>
</Card>
```

## Calculator example

```svelte
<script lang="ts">
  import { Button, Card, CardContent, CardHeader, CardTitle, Input } from '@omni/ui'

  let units = $state(1000)
  let price = $state(25)
  let total = $derived(units * price)
</script>

<Card class="mx-auto max-w-md">
  <CardHeader><CardTitle>Revenue projection</CardTitle></CardHeader>
  <CardContent class="space-y-4">
    <label class="block space-y-1 text-sm">Units <Input type="number" bind:value={units} min="0" /></label>
    <label class="block space-y-1 text-sm">Price <Input type="number" bind:value={price} min="0" step="0.01" /></label>
    <p class="text-2xl font-semibold">${total.toLocaleString()}</p>
    <Button class="cursor-pointer" onclick={() => (units += 100)}>Add 100 units</Button>
  </CardContent>
</Card>
```
