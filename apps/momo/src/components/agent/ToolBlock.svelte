<!--
  ToolBlock — compact white pill showing what the agent is doing right now.

  Anatomy (Paper artboard 1XD-0):
    - height 36px, padding 8px 14px, gap 10px
    - radius 12px, white fill, 1px solid var(--color-input-border) (#E0E0E0)
    - leading 16px Lucide icon, label 13px / 16px system regular #525252
    - width: fit-content so the block hugs the label rather than stretching

  Icon name is the same IconName union used elsewhere — we resolve via the
  same minimal map (only calendar / phone are referenced today, but the map
  is exhaustive so future tools don't fall through silently).
-->
<script lang="ts">
  import {
    Bot,
    Cake,
    Calendar,
    Check,
    CircleAlert,
    CreditCard,
    Mail,
    MessageCircle,
    Phone,
    ShoppingBag,
    User,
    Users,
    Wallet,
    Briefcase
  } from "lucide-svelte";

  import type { IconName } from "../../data/types";

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  type IconComponent = any;

  const iconMap: Record<IconName, IconComponent> = {
    calendar: Calendar,
    "message-circle": MessageCircle,
    cake: Cake,
    "shopping-bag": ShoppingBag,
    mail: Mail,
    phone: Phone,
    wallet: Wallet,
    "credit-card": CreditCard,
    user: User,
    users: Users,
    briefcase: Briefcase,
    bot: Bot,
    check: Check,
    "circle-alert": CircleAlert
  };

  interface Props {
    icon: IconName;
    label: string;
  }

  let { icon, label }: Props = $props();

  let Glyph = $derived(iconMap[icon] ?? Bot);
</script>

<div class="tool-block">
  <Glyph size={16} strokeWidth={1.6} aria-hidden="true" />
  <span class="tool-block__label">{label}</span>
</div>

<style>
  .tool-block {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 8px 14px;
    border-radius: 12px;
    background: var(--color-surface-app);
    border: 1px solid var(--color-input-border);
    color: var(--color-text-secondary);
    width: fit-content;
    max-width: 100%;
  }

  .tool-block__label {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-regular);
    color: var(--color-text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
