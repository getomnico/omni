<script lang="ts">
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import MaskedCredentialInput from '$lib/components/masked-credential-input.svelte'

    interface Props {
        serviceAccountJson?: string
        principalEmail?: string
        domain?: string
        hasStoredKey?: boolean
        disabled?: boolean
        // 'sa-direct' hides the admin-email/domain fields and shows
        // non-impersonation guidance. Defaults to DWD behavior.
        mode?: 'dwd' | 'sa-direct'
    }

    let {
        serviceAccountJson = $bindable(''),
        principalEmail = $bindable(''),
        domain = $bindable(''),
        hasStoredKey = false,
        disabled = false,
        mode = 'dwd',
    }: Props = $props()

    const isSaDirect = $derived(mode === 'sa-direct')
</script>

<div class="space-y-4">
    <div class="space-y-2">
        <Label for="service-account-json">Service Account JSON Key</Label>
        <MaskedCredentialInput
            id="service-account-json"
            bind:value={serviceAccountJson}
            hasStoredValue={hasStoredKey}
            multiline
            {disabled}
            placeholder="Paste your Google service account JSON key here..." />
        <p class="text-muted-foreground text-sm">
            Download this from the Google Cloud Console under "Service Accounts" > "Keys".
        </p>
    </div>

    {#if isSaDirect}
        <p class="text-muted-foreground text-sm">
            The service account will authenticate as itself — no user impersonation, no domain-wide
            delegation. Add the service account email to each shared drive as
            <span class="font-medium">Content manager</span> or
            <span class="font-medium">Manager</span>.
        </p>
    {:else}
        <div class="space-y-2">
            <Label for="principal-email">Admin Email</Label>
            <Input
                id="principal-email"
                name="principalEmail"
                bind:value={principalEmail}
                placeholder="admin@yourdomain.com"
                type="email"
                {disabled}
                required />
            <p class="text-muted-foreground text-sm">
                The admin user email that the service account will impersonate to access Google
                Workspace APIs.
            </p>
        </div>

        <div class="space-y-2">
            <Label for="domain">Organization Domain</Label>
            <Input
                id="domain"
                name="domain"
                bind:value={domain}
                placeholder="yourdomain.com"
                type="text"
                {disabled}
                required />
            <p class="text-muted-foreground text-sm">
                Your Google Workspace domain (e.g., company.com). The service account will
                impersonate all users in this domain.
            </p>
        </div>
    {/if}
</div>
