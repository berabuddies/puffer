<!--
  EmailConnectModal — one-shot IMAP/SMTP credential form.

  Submitting the form fires `emailConfigure` which spawns
  `puffer connect email configure …` server-side. The puffer subscriber
  writes the credentials to `~/.puffer/subscribers/email/state/config.toml`
  and the modal toasts the result. Ports may be omitted: `0` tells the
  subscriber to use IMAP=993 / SMTP=587 defaults.
-->
<script lang="ts">
  import Modal from "../common/Modal.svelte";
  import Button from "../common/Button.svelte";
  import { emailConfigure } from "../../lib/connectorClient";
  import { pushToast } from "../../lib/toast.svelte";

  interface Props {
    open: boolean;
    onClose: () => void;
  }

  let { open, onClose }: Props = $props();

  let imapHost = $state("");
  let imapPort = $state("");
  let smtpHost = $state("");
  let smtpPort = $state("");
  let username = $state("");
  let password = $state("");
  let fromAddress = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    if (!open) {
      imapHost = "";
      imapPort = "";
      smtpHost = "";
      smtpPort = "";
      username = "";
      password = "";
      fromAddress = "";
      busy = false;
      error = null;
    }
  });

  function parsePort(raw: string, field: string): number | string {
    const trimmed = raw.trim();
    if (trimmed === "") return 0;
    const parsed = Number(trimmed);
    if (!Number.isInteger(parsed) || parsed < 0 || parsed > 65535) {
      return `${field} must be 0–65535.`;
    }
    return parsed;
  }

  async function submit(): Promise<void> {
    error = null;
    if (!imapHost.trim() || !smtpHost.trim()) {
      error = "IMAP and SMTP hostnames are required.";
      return;
    }
    if (!username.trim() || !password) {
      error = "Username and password are required.";
      return;
    }
    if (!fromAddress.trim()) {
      error = "From address is required.";
      return;
    }
    const imapPortValue = parsePort(imapPort, "IMAP port");
    if (typeof imapPortValue !== "number") {
      error = imapPortValue;
      return;
    }
    const smtpPortValue = parsePort(smtpPort, "SMTP port");
    if (typeof smtpPortValue !== "number") {
      error = smtpPortValue;
      return;
    }
    busy = true;
    try {
      await emailConfigure({
        imapHost: imapHost.trim(),
        imapPort: imapPortValue,
        smtpHost: smtpHost.trim(),
        smtpPort: smtpPortValue,
        username: username.trim(),
        password,
        fromAddress: fromAddress.trim(),
      });
      pushToast("Email connected", "success");
      onClose();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<Modal {open} {onClose} title="Connect Email" maxWidth="460px">
  <form
    class="em-body"
    onsubmit={(e) => {
      e.preventDefault();
      void submit();
    }}
  >
    <p class="em-help">
      Configure IMAP for receiving and SMTP for sending. Use an app password if
      your provider requires one (e.g. Gmail, iCloud).
    </p>

    <label class="em-field">
      <span class="em-label">Email address</span>
      <input
        type="email"
        inputmode="email"
        autocomplete="email"
        placeholder="you@example.com"
        bind:value={fromAddress}
        disabled={busy}
      />
    </label>

    <label class="em-field">
      <span class="em-label">Username</span>
      <input
        type="text"
        autocomplete="username"
        placeholder="Usually your email address"
        bind:value={username}
        disabled={busy}
      />
    </label>

    <label class="em-field">
      <span class="em-label">Password</span>
      <input
        type="password"
        autocomplete="current-password"
        bind:value={password}
        disabled={busy}
      />
    </label>

    <div class="em-grid">
      <label class="em-field">
        <span class="em-label">IMAP host</span>
        <input
          type="text"
          placeholder="imap.example.com"
          bind:value={imapHost}
          disabled={busy}
        />
      </label>
      <label class="em-field em-field--port">
        <span class="em-label">Port</span>
        <input
          type="number"
          min="0"
          max="65535"
          placeholder="993"
          bind:value={imapPort}
          disabled={busy}
        />
      </label>
    </div>

    <div class="em-grid">
      <label class="em-field">
        <span class="em-label">SMTP host</span>
        <input
          type="text"
          placeholder="smtp.example.com"
          bind:value={smtpHost}
          disabled={busy}
        />
      </label>
      <label class="em-field em-field--port">
        <span class="em-label">Port</span>
        <input
          type="number"
          min="0"
          max="65535"
          placeholder="587"
          bind:value={smtpPort}
          disabled={busy}
        />
      </label>
    </div>

    {#if error}<p class="em-error">{error}</p>{/if}

    <div class="em-actions">
      <Button
        label="Cancel"
        variant="secondary"
        size="md"
        onclick={onClose}
        disabled={busy}
        type="button"
      />
      <Button
        label={busy ? "Saving…" : "Save"}
        variant="primary"
        size="md"
        disabled={busy}
        type="submit"
      />
    </div>
  </form>
</Modal>

<style>
  .em-body {
    padding: var(--space-4) 20px 20px 20px;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .em-help {
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    color: var(--color-text-secondary);
  }

  .em-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .em-label {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
  }

  .em-field input {
    height: 40px;
    border: var(--border-hairline);
    border-radius: 10px;
    padding: 0 12px;
    font-family: var(--font-system);
    font-size: 14px;
    color: var(--color-text-primary);
    background: var(--color-surface-input);
  }

  .em-field input:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 1px;
  }

  .em-field input:disabled {
    opacity: 0.55;
  }

  .em-grid {
    display: grid;
    grid-template-columns: 1fr 110px;
    gap: var(--space-3);
  }

  .em-error {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    color: #b3261e;
  }

  .em-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-3);
  }
</style>
