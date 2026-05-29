<!--
  KycHelpModal — "how to fill in your details" help dialog for the KYC form.

  Opened from the trigger in WalletKycForm's title row. Reuses the common
  Modal primitive (ESC / backdrop-click close + body-scroll lock) and just
  renders a static per-field tip list in the slot.

  Content scope: only the fields that exist on the momo KYC form. The PDF
  source (TC37 — KYC verification Requirements / "Stradacarte Portal –
  Card holder Creation") also lists middle name, nationality, place of
  birth, occupation and an "Address 2" line, but the momo form has no such
  inputs, so those are intentionally omitted. Where the PDF rules conflict
  with prior momo behaviour the PDF wins — notably the email rules, which
  are now also enforced by ../../lib/kycValidation.
-->
<script lang="ts">
  import Modal from "../common/Modal.svelte";

  interface Props {
    open: boolean;
    onClose: () => void;
  }

  let { open, onClose }: Props = $props();

  // Per-field guidance, in the same top-to-bottom order as the form. Wording
  // follows the PDF; fields without a PDF-specific rule get a short, plain
  // "what goes here" hint so every input is covered.
  const TIPS: ReadonlyArray<{ field: string; tip: string }> = [
    {
      field: "First / Last name",
      tip: "Enter your legal name exactly as it appears on your government ID."
    },
    {
      field: "Date of birth",
      tip: "Use the date of birth shown on your ID. You must be at least 18."
    },
    {
      field: "Email",
      tip: "Max 36 characters. Do not include a “+” in the email."
    },
    {
      field: "Country",
      tip: "Select your country from the dropdown."
    },
    {
      field: "Code",
      tip: "Your country calling code — auto-filled when you pick a country."
    },
    {
      field: "Phone number",
      tip: "Max 10 digits. Don’t add the country calling code — use the Code field for that."
    },
    {
      field: "Address",
      tip: "Enter your complete address (max 50 characters)."
    },
    {
      field: "City",
      tip: "The city on your proof of address."
    },
    {
      field: "State / Region",
      tip: "For the US, pick your state; otherwise enter your region / province."
    },
    {
      field: "Zip / Postal",
      tip: "Your postal / ZIP code."
    }
  ];
</script>

<Modal {open} {onClose} title="How to fill in your details" maxWidth="460px">
  <div class="kyc-help">
    <p class="kyc-help__intro">
      A few fields have specific rules. Here’s how to fill each one so your
      verification isn’t rejected.
    </p>
    <dl class="kyc-help__list">
      {#each TIPS as t (t.field)}
        <div class="kyc-help__row">
          <dt class="kyc-help__field">{t.field}</dt>
          <dd class="kyc-help__tip">{t.tip}</dd>
        </div>
      {/each}
    </dl>
  </div>
</Modal>

<style>
  /* Modal renders the title header itself; this is the scrollable body.
     Horizontal padding matches the header (20px) so text lines up. */
  .kyc-help {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 16px 20px 20px 20px;
    overflow-y: auto;
  }

  .kyc-help__intro {
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    color: var(--color-text-secondary);
  }

  .kyc-help__list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .kyc-help__row {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .kyc-help__field {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
  }

  .kyc-help__tip {
    margin: 0;
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    color: var(--color-text-secondary);
  }
</style>
