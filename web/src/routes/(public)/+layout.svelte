<script lang="ts">
	import '../../app.css';
</script>

<div class="min-h-screen bg-gradient-to-br from-slate-50 to-slate-100 dark:from-slate-900 dark:to-slate-800">
	<div class="flex min-h-screen items-center justify-center p-4">
		<div class="w-full max-w-md">
			<div class="mb-8 text-center">
				<h1 class="text-3xl font-bold text-slate-900 dark:text-slate-100">Clio</h1>
				<p class="mt-2 text-slate-600 dark:text-slate-400">Enterprise Search Platform</p>
			</div>
			<slot />
		</div>
	</div>
</div>