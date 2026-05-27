<!--
  Connected Apps — the catalogue of integrations Momo can act through.

  Only the apps Momo can authenticate end-to-end live here today:
    Telegram (personal-account login) and Email (IMAP/SMTP).

  Click handling:
    - Card status "not_connected" → open the matching connect modal.
    - Card status "connected"     → open the disconnect confirm modal;
                                    confirming calls connectorDisconnect
                                    which wipes the credentials on disk.

  Card status is refreshed via `connector_status` on mount and after
  every modal close so it reflects what's actually persisted under
  `~/.puffer/` (telegram `credentials.json` / email `config.toml`).
-->
<script lang="ts">
  import Composer from "../components/shell/Composer.svelte";
  import AppCard from "../components/apps/AppCard.svelte";
  import TelegramConnectModal from "../components/apps/TelegramConnectModal.svelte";
  import EmailConnectModal from "../components/apps/EmailConnectModal.svelte";
  import ConfirmDisconnectModal from "../components/apps/ConfirmDisconnectModal.svelte";
  import { apps as seedApps } from "../data/apps";
  import type { AppEntry } from "../data/types";
  import {
    connectorDisconnect,
    getConnectorStatus,
    type ConnectorKind,
  } from "../lib/connectorClient";
  import { pushToast } from "../lib/toast.svelte";

  let apps = $state<AppEntry[]>(seedApps.map((a) => ({ ...a })));

  let telegramOpen = $state(false);
  let emailOpen = $state(false);
  let disconnectTarget = $state<AppEntry | null>(null);
  let disconnectBusy = $state(false);

  void refreshStatus();

  async function refreshStatus(): Promise<void> {
    try {
      const status = await getConnectorStatus();
      for (const app of apps) {
        if (app.connectorSlug === "telegram-login") {
          app.status = status.telegram ? "connected" : "not_connected";
        } else if (app.connectorSlug === "email") {
          app.status = status.email ? "connected" : "not_connected";
        }
      }
    } catch {
      // Backend not ready yet — leave seed values. Re-tries on every
      // modal close, so a transient WS hiccup at boot resolves itself.
    }
  }

  function kindFor(entry: AppEntry): ConnectorKind {
    return entry.connectorSlug === "email" ? "email" : "telegram";
  }

  function openCard(id: string): void {
    const entry = apps.find((a) => a.id === id);
    if (!entry) return;
    if (entry.status === "connected") {
      disconnectTarget = entry;
      return;
    }
    if (entry.connectorSlug === "telegram-login") telegramOpen = true;
    else if (entry.connectorSlug === "email") emailOpen = true;
  }

  function closeTelegram(): void {
    telegramOpen = false;
    void refreshStatus();
  }

  function closeEmail(): void {
    emailOpen = false;
    void refreshStatus();
  }

  function closeDisconnect(): void {
    if (disconnectBusy) return;
    disconnectTarget = null;
  }

  async function confirmDisconnect(): Promise<void> {
    const target = disconnectTarget;
    if (!target || disconnectBusy) return;
    disconnectBusy = true;
    try {
      await connectorDisconnect(kindFor(target));
      pushToast(`${target.name} disconnected`, "info");
      disconnectTarget = null;
      await refreshStatus();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      pushToast(`Could not disconnect: ${message}`, "error");
    } finally {
      disconnectBusy = false;
    }
  }
</script>

<header class="apps-header">
  <h1 class="text-section">Connected Apps</h1>
</header>

<section class="apps-grid" aria-label="Connected apps">
  {#each apps as app (app.id)}
    <AppCard {app} onToggle={openCard} />
  {/each}
</section>

<div class="apps-spacer"></div>

<Composer placeholder="Send a message to chaofan" />

<TelegramConnectModal open={telegramOpen} onClose={closeTelegram} />
<EmailConnectModal open={emailOpen} onClose={closeEmail} />
<ConfirmDisconnectModal
  open={disconnectTarget !== null}
  name={disconnectTarget?.name ?? ""}
  busy={disconnectBusy}
  onConfirm={confirmDisconnect}
  onClose={closeDisconnect}
/>

<style>
  .apps-header {
    padding-bottom: var(--space-5);
  }

  .apps-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    column-gap: var(--space-4);
    row-gap: var(--space-4);
  }

  .apps-spacer {
    flex: 1;
    min-height: var(--space-5);
  }
</style>
