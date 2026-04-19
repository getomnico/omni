<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = $bindable(false), onSuccess, onCancel }: Props = $props()

    let mode = $state<'telethon' | 'bot'>('telethon')
    // Telethon has two sub-modes: full interactive flow (phone → SMS → 2FA)
    // or paste a pre-generated session string from `scripts/auth.py`.
    let telethonMethod = $state<'interactive' | 'paste'>('interactive')
    let apiId = $state('')
    let apiHash = $state('')
    let sessionString = $state('')
    let botToken = $state('')
    let isSubmitting = $state(false)

    // Interactive auth state
    type AuthStep = 'start' | 'code' | 'twofa' | 'done'
    let authStep = $state<AuthStep>('start')
    let phone = $state('')
    let code = $state('')
    let password = $state('')
    let partialSession = $state('')
    let phoneCodeHash = $state('')
    let authedUser = $state<{ display_name?: string; username?: string } | null>(null)
    let isAuthing = $state(false)
    let authError = $state<string | null>(null)

    async function callAuthAction(
        action: 'auth_send_code' | 'auth_verify_code',
        params: Record<string, unknown>,
    ): Promise<Record<string, unknown>> {
        const res = await fetch('/api/connectors/telegram/auth', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ action, params }),
        })
        const body = await res.json().catch(() => null)
        if (!res.ok || !body?.ok) {
            throw new Error(body?.error || body?.message || `Auth call failed (${res.status})`)
        }
        return (body.result ?? {}) as Record<string, unknown>
    }

    async function sendCode() {
        authError = null
        if (!apiId.trim() || !apiHash.trim() || !phone.trim()) {
            authError = 'API ID, API Hash, and phone are required'
            return
        }
        if (!/^\d+$/.test(apiId.trim())) {
            authError = 'API ID must be a number'
            return
        }
        isAuthing = true
        try {
            const result = await callAuthAction('auth_send_code', {
                api_id: Number(apiId.trim()),
                api_hash: apiHash.trim(),
                phone: phone.trim(),
            })
            partialSession = String(result.partial_session ?? '')
            phoneCodeHash = String(result.phone_code_hash ?? '')
            authStep = 'code'
        } catch (err: any) {
            authError = err.message || 'Failed to send code'
        } finally {
            isAuthing = false
        }
    }

    async function verifyCode() {
        authError = null
        if (!code.trim()) {
            authError = 'Enter the code Telegram sent you'
            return
        }
        isAuthing = true
        try {
            const result = await callAuthAction('auth_verify_code', {
                api_id: Number(apiId.trim()),
                api_hash: apiHash.trim(),
                partial_session: partialSession,
                phone: phone.trim(),
                phone_code_hash: phoneCodeHash,
                code: code.trim(),
            })
            if (result.needs_2fa) {
                // Telegram demands the 2FA password — re-use the in-flight
                // partial session so we don't have to re-send a new SMS.
                partialSession = String(result.partial_session ?? partialSession)
                authStep = 'twofa'
                return
            }
            sessionString = String(result.session ?? '')
            authedUser = (result.user as { display_name?: string; username?: string }) ?? null
            authStep = 'done'
        } catch (err: any) {
            authError = err.message || 'Failed to verify code'
        } finally {
            isAuthing = false
        }
    }

    async function verifyPassword() {
        authError = null
        if (!password) {
            authError = 'Enter your 2FA password'
            return
        }
        isAuthing = true
        try {
            const result = await callAuthAction('auth_verify_code', {
                api_id: Number(apiId.trim()),
                api_hash: apiHash.trim(),
                partial_session: partialSession,
                phone: phone.trim(),
                phone_code_hash: phoneCodeHash,
                code: code.trim(),
                password,
            })
            sessionString = String(result.session ?? '')
            authedUser = (result.user as { display_name?: string; username?: string }) ?? null
            authStep = 'done'
        } catch (err: any) {
            authError = err.message || 'Failed to verify password'
        } finally {
            isAuthing = false
        }
    }

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (mode === 'telethon') {
                if (!apiId.trim() || !apiHash.trim() || !sessionString.trim()) {
                    throw new Error(
                        telethonMethod === 'interactive'
                            ? 'Complete the phone verification steps first'
                            : 'API ID, API Hash, and Session String are all required',
                    )
                }
                if (!/^\d+$/.test(apiId.trim())) {
                    throw new Error('API ID must be a number')
                }
            } else {
                if (!botToken.trim()) {
                    throw new Error('Bot token is required')
                }
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'Telegram',
                    sourceType: 'telegram',
                    config: {},
                }),
            })

            if (!sourceResponse.ok) {
                const err = await sourceResponse.json().catch(() => null)
                throw new Error(err?.message || 'Failed to create Telegram source')
            }

            const source = await sourceResponse.json()

            const credentials =
                mode === 'telethon'
                    ? {
                          api_id: apiId.trim(),
                          api_hash: apiHash.trim(),
                          session: sessionString.trim(),
                      }
                    : { token: botToken.trim() }

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'telegram',
                    authType: mode === 'telethon' ? AuthType.API_KEY : AuthType.BOT_TOKEN,
                    credentials,
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to store Telegram credentials')
            }

            toast.success('Telegram connected! Configure chat selection in settings.')
            open = false
            resetForm()
            onSuccess?.()
        } catch (error: any) {
            console.error('Error setting up Telegram:', error)
            toast.error(error.message || 'Failed to set up Telegram')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        open = false
        resetForm()
        onCancel?.()
    }

    function resetForm() {
        apiId = ''
        apiHash = ''
        sessionString = ''
        botToken = ''
        mode = 'telethon'
        telethonMethod = 'interactive'
        authStep = 'start'
        phone = ''
        code = ''
        password = ''
        partialSession = ''
        phoneCodeHash = ''
        authedUser = null
        authError = null
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Telegram</Dialog.Title>
            <Dialog.Description>
                Index messages from Telegram chats, groups, and channels.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <!-- Mode selector -->
            <div class="space-y-2">
                <Label>Connection Mode</Label>
                <div class="flex gap-4">
                    <label class="flex cursor-pointer items-center gap-2">
                        <input
                            type="radio"
                            name="mode"
                            value="telethon"
                            bind:group={mode}
                            class="accent-primary" />
                        <span class="text-sm font-medium">User Session (full history)</span>
                    </label>
                    <label class="flex cursor-pointer items-center gap-2">
                        <input
                            type="radio"
                            name="mode"
                            value="bot"
                            bind:group={mode}
                            class="accent-primary" />
                        <span class="text-sm font-medium">Bot Token (forward-only)</span>
                    </label>
                </div>
            </div>

            {#if mode === 'telethon'}
                <!-- API credentials (shared by both sub-methods) -->
                <div class="space-y-2">
                    <Label for="api-id">API ID</Label>
                    <Input
                        id="api-id"
                        bind:value={apiId}
                        placeholder="12345678"
                        disabled={authStep !== 'start'}
                        required />
                    <p class="text-muted-foreground text-sm">
                        Get your API ID and API Hash from <a
                            href="https://my.telegram.org/apps"
                            target="_blank"
                            class="text-blue-600 hover:underline">my.telegram.org/apps</a>
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="api-hash">API Hash</Label>
                    <Input
                        id="api-hash"
                        bind:value={apiHash}
                        placeholder="a1b2c3d4e5f6..."
                        type="password"
                        disabled={authStep !== 'start'}
                        required />
                </div>

                <!-- Sub-method toggle -->
                <div class="space-y-2">
                    <Label>How do you want to sign in?</Label>
                    <div class="flex gap-4">
                        <label class="flex cursor-pointer items-center gap-2">
                            <input
                                type="radio"
                                name="telethon-method"
                                value="interactive"
                                bind:group={telethonMethod}
                                class="accent-primary" />
                            <span class="text-sm font-medium"
                                >Sign in with phone number (recommended)</span>
                        </label>
                        <label class="flex cursor-pointer items-center gap-2">
                            <input
                                type="radio"
                                name="telethon-method"
                                value="paste"
                                bind:group={telethonMethod}
                                class="accent-primary" />
                            <span class="text-sm font-medium">Paste existing session string</span>
                        </label>
                    </div>
                </div>

                {#if telethonMethod === 'interactive'}
                    <!-- Step 1: phone -->
                    {#if authStep === 'start'}
                        <div class="space-y-2">
                            <Label for="phone">Phone number</Label>
                            <Input
                                id="phone"
                                bind:value={phone}
                                placeholder="+1 555 123 4567"
                                required />
                            <p class="text-muted-foreground text-sm">
                                Include the country code. Telegram will send a login code to this
                                number.
                            </p>
                        </div>
                        <Button
                            onclick={sendCode}
                            disabled={isAuthing}
                            class="w-full cursor-pointer">
                            {isAuthing ? 'Sending code…' : 'Send code'}
                        </Button>
                    {/if}

                    <!-- Step 2: SMS code -->
                    {#if authStep === 'code'}
                        <div class="bg-muted/30 rounded-md p-3 text-sm">
                            Code sent to <strong>{phone}</strong>. Check your Telegram app (or
                            SMS if no active Telegram device).
                        </div>
                        <div class="space-y-2">
                            <Label for="login-code">Login code</Label>
                            <Input
                                id="login-code"
                                bind:value={code}
                                placeholder="12345"
                                autocomplete="one-time-code"
                                required />
                        </div>
                        <div class="flex gap-2">
                            <Button
                                variant="outline"
                                onclick={() => {
                                    authStep = 'start'
                                    code = ''
                                    authError = null
                                }}
                                disabled={isAuthing}
                                class="cursor-pointer">Back</Button>
                            <Button
                                onclick={verifyCode}
                                disabled={isAuthing}
                                class="flex-1 cursor-pointer">
                                {isAuthing ? 'Verifying…' : 'Verify'}
                            </Button>
                        </div>
                    {/if}

                    <!-- Step 3: 2FA password (only if the account has it enabled) -->
                    {#if authStep === 'twofa'}
                        <div class="bg-muted/30 rounded-md p-3 text-sm">
                            Two-factor authentication is enabled on this account. Enter your
                            Telegram password to finish signing in.
                        </div>
                        <div class="space-y-2">
                            <Label for="twofa-password">Two-factor password</Label>
                            <Input
                                id="twofa-password"
                                bind:value={password}
                                type="password"
                                required />
                        </div>
                        <Button
                            onclick={verifyPassword}
                            disabled={isAuthing}
                            class="w-full cursor-pointer">
                            {isAuthing ? 'Verifying…' : 'Sign in'}
                        </Button>
                    {/if}

                    <!-- Step 4: success -->
                    {#if authStep === 'done'}
                        <div class="rounded-md border border-green-500/40 bg-green-500/10 p-3 text-sm">
                            Signed in as
                            <strong>
                                {authedUser?.display_name || authedUser?.username || 'Telegram user'}
                            </strong>. Click Connect below to save this source.
                        </div>
                    {/if}

                    {#if authError}
                        <div class="rounded-md border border-red-500/40 bg-red-500/10 p-3 text-sm text-red-600">
                            {authError}
                        </div>
                    {/if}
                {:else}
                    <!-- Paste pre-generated session string (fallback for CI / scripts) -->
                    <div class="space-y-2">
                        <Label for="session-string">Session String</Label>
                        <Input
                            id="session-string"
                            bind:value={sessionString}
                            placeholder="1BVtsO..."
                            type="password"
                            required />
                        <p class="text-muted-foreground text-sm">
                            Generate offline:
                            <code class="bg-muted rounded px-1 text-xs"
                                >python connectors/telegram/scripts/auth.py --api-id YOUR_ID --api-hash YOUR_HASH</code>
                        </p>
                    </div>
                {/if}
            {:else}
                <div class="space-y-2">
                    <Label for="bot-token">Bot Token</Label>
                    <Input
                        id="bot-token"
                        bind:value={botToken}
                        placeholder="123456:ABC-DEF..."
                        type="password"
                        required />
                    <p class="text-muted-foreground text-sm">
                        Create a bot via <a
                            href="https://t.me/BotFather"
                            target="_blank"
                            class="text-blue-600 hover:underline">@BotFather</a>
                        and copy the token. Note: Bot API only indexes messages received after
                        setup.
                    </p>
                </div>
            {/if}
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button
                onclick={handleSubmit}
                disabled={isSubmitting ||
                    (mode === 'telethon' &&
                        telethonMethod === 'interactive' &&
                        authStep !== 'done')}
                class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
