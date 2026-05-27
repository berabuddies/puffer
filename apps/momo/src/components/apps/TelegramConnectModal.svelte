<!--
  TelegramConnectModal — three-step Telegram personal-account login.

  Flow:
    phone → backend spawns `puffer connect telegram`, sends login_start,
            returns the first event (awaiting_code | awaiting_password |
            complete | failed).
    code  → submit_code RPC.
    password (only if Telegram requested 2FA).
    complete → toast + onClose.

  The backend keeps the puffer subprocess alive across these steps via
  the sessionId we got from `telegramStart`. If the user closes the
  modal mid-flow we cancel the session so the orphaned child gets
  killed.
-->
<script lang="ts">
  import Modal from "../common/Modal.svelte";
  import Button from "../common/Button.svelte";
  import {
    telegramStart,
    telegramSubmitCode,
    telegramSubmitPassword,
    telegramCancel,
    type ConnectorEvent,
    type TelegramSessionResult,
  } from "../../lib/connectorClient";
  import { pushToast } from "../../lib/toast.svelte";

  interface Props {
    open: boolean;
    onClose: () => void;
  }

  let { open, onClose }: Props = $props();

  type Step = "phone" | "code" | "password" | "complete";

  let step = $state<Step>("phone");
  let phone = $state("");
  let code = $state("");
  let password = $state("");
  let sessionId = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    if (!open) {
      if (sessionId && step !== "complete") {
        void telegramCancel(sessionId).catch(() => undefined);
      }
      step = "phone";
      phone = "";
      code = "";
      password = "";
      sessionId = null;
      busy = false;
      error = null;
    }
  });

  function eventError(event: ConnectorEvent | undefined): string {
    if (!event) return "Telegram did not respond.";
    const payload = event.payload ?? {};
    const message = typeof payload["error"] === "string" ? (payload["error"] as string) : "";
    if (message) return message;
    return `Telegram connect failed (${event.kind || "unknown"}).`;
  }

  function applyResult(result: TelegramSessionResult): void {
    sessionId = result.sessionId;
    switch (result.status) {
      case "awaiting_code":
        step = "code";
        break;
      case "awaiting_password":
        step = "password";
        break;
      case "complete":
        step = "complete";
        pushToast("Telegram connected", "success");
        break;
      case "failed":
      default:
        error = eventError(result.event);
    }
  }

  async function submitPhone(): Promise<void> {
    const trimmed = phone.trim();
    if (!trimmed) {
      error = "Phone number is required.";
      return;
    }
    busy = true;
    error = null;
    try {
      applyResult(await telegramStart({ phone: trimmed }));
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }

  async function submitCode(): Promise<void> {
    if (!sessionId) return;
    const trimmed = code.trim();
    if (!trimmed) {
      error = "Code is required.";
      return;
    }
    busy = true;
    error = null;
    try {
      applyResult(await telegramSubmitCode({ sessionId, code: trimmed }));
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }

  async function submitPassword(): Promise<void> {
    if (!sessionId) return;
    if (!password) {
      error = "Password is required.";
      return;
    }
    busy = true;
    error = null;
    try {
      applyResult(await telegramSubmitPassword({ sessionId, password }));
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<Modal {open} {onClose} title="Connect Telegram" maxWidth="420px">
  <div class="tg-body">
    {#if step === "phone"}
      <p class="tg-help">
        Enter the phone number for your Telegram account. We'll text you a one-time login code.
      </p>
      <label class="tg-field">
        <span class="tg-label">Phone number</span>
        <input
          type="tel"
          inputmode="tel"
          autocomplete="tel"
          placeholder="+1 555 123 4567"
          bind:value={phone}
          disabled={busy}
        />
      </label>
      {#if error}<p class="tg-error">{error}</p>{/if}
      <div class="tg-actions">
        <Button label="Cancel" variant="secondary" size="md" onclick={onClose} disabled={busy} />
        <Button
          label={busy ? "Sending…" : "Send code"}
          variant="primary"
          size="md"
          onclick={submitPhone}
          disabled={busy}
        />
      </div>
    {:else if step === "code"}
      <p class="tg-help">
        Telegram sent a login code to <strong>{phone}</strong>. Enter it below.
      </p>
      <label class="tg-field">
        <span class="tg-label">Login code</span>
        <input
          type="text"
          inputmode="numeric"
          autocomplete="one-time-code"
          placeholder="12345"
          bind:value={code}
          disabled={busy}
        />
      </label>
      {#if error}<p class="tg-error">{error}</p>{/if}
      <div class="tg-actions">
        <Button label="Cancel" variant="secondary" size="md" onclick={onClose} disabled={busy} />
        <Button
          label={busy ? "Verifying…" : "Verify"}
          variant="primary"
          size="md"
          onclick={submitCode}
          disabled={busy}
        />
      </div>
    {:else if step === "password"}
      <p class="tg-help">
        Your account has 2-step verification on. Enter your Telegram cloud password.
      </p>
      <label class="tg-field">
        <span class="tg-label">Cloud password</span>
        <input
          type="password"
          autocomplete="current-password"
          bind:value={password}
          disabled={busy}
        />
      </label>
      {#if error}<p class="tg-error">{error}</p>{/if}
      <div class="tg-actions">
        <Button label="Cancel" variant="secondary" size="md" onclick={onClose} disabled={busy} />
        <Button
          label={busy ? "Signing in…" : "Sign in"}
          variant="primary"
          size="md"
          onclick={submitPassword}
          disabled={busy}
        />
      </div>
    {:else}
      <p class="tg-help">Telegram is connected. You can close this dialog.</p>
      <div class="tg-actions">
        <Button label="Done" variant="primary" size="md" onclick={onClose} />
      </div>
    {/if}
  </div>
</Modal>

<style>
  .tg-body {
    padding: var(--space-4) 20px 20px 20px;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .tg-help {
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    color: var(--color-text-secondary);
  }

  .tg-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .tg-label {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
  }

  .tg-field input {
    height: 40px;
    border: var(--border-hairline);
    border-radius: 10px;
    padding: 0 12px;
    font-family: var(--font-system);
    font-size: 14px;
    color: var(--color-text-primary);
    background: var(--color-surface-input);
  }

  .tg-field input:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 1px;
  }

  .tg-field input:disabled {
    opacity: 0.55;
  }

  .tg-error {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    color: #b3261e;
  }

  .tg-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-3);
  }
</style>
