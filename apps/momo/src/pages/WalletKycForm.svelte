<!--
  Wallet · KYC form (route /wallet/kyc/form).

  Collects the 11-field cardholder payload defined by `CreateCardHolderParams`
  in walletTypes.ts. Layout is a single column of label/input pairs inside
  the 760 page column; the submit cluster sits at the bottom of the form.

  Validation is delegated to ../lib/kycValidation, the production validator
  ported from worldclaw-app. `validateForm` returns `{ errors, invalidKeys }`
  where `errors` is a Partial<Record<KycFieldKey, string>>; we adapt it at
  the call site into the per-field error map this template renders against.

  On successful submit the ekycUrl from `walletClient.startKyc(form)` is
  cached in `walletState.ekycUrl` and we navigate to the verify page.
-->
<script lang="ts">
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";
  import { walletClient, walletState } from "../lib/walletClient.svelte";
  import {
    validateForm,
    US_STATE_CODES,
    type KycFieldKey,
    type KycFormShape
  } from "../lib/kycValidation";
  import type { CreateCardHolderParams } from "../lib/walletTypes";
  import Button from "../components/common/Button.svelte";
  import KycHelpModal from "../components/wallet/KycHelpModal.svelte";
  import { Info } from "lucide-svelte";

  type KycErrors = Partial<Record<KycFieldKey, string>>;
  type TouchedMap = Partial<Record<KycFieldKey, boolean>>;

  // Smallest country list that covers the common cases for demo. The real
  // implementation will source this from a static asset; the UI just needs
  // a 2-letter ISO code to submit.
  const COUNTRIES: Array<{ code: string; name: string; callingCode: string }> = [
    { code: "US", name: "United States", callingCode: "+1" },
    { code: "CA", name: "Canada", callingCode: "+1" },
    { code: "GB", name: "United Kingdom", callingCode: "+44" },
    { code: "JP", name: "Japan", callingCode: "+81" },
    { code: "CN", name: "China", callingCode: "+86" },
    { code: "SG", name: "Singapore", callingCode: "+65" },
    { code: "HK", name: "Hong Kong", callingCode: "+852" },
    { code: "AU", name: "Australia", callingCode: "+61" }
  ];

  // Mirror the validator's accepted set so the dropdown can't produce a
  // value the server-side checker would reject (e.g. "ca" vs "CA").
  const US_STATES = US_STATE_CODES;

  // Empty initial form — mirrors worldclaw-app EMPTY_FORM. The Submit
  // button stays disabled until every required field has content, so we
  // don't need a "valid by default" pre-seed any more.
  let form = $state<CreateCardHolderParams>({
    firstName: "",
    lastName: "",
    dob: "",
    email: "",
    callingCode: "",
    countryCallingCode: "",
    phoneNum: "",
    cellNum: "",
    address: "",
    city: "",
    state: "",
    country: "",
    zipCode: ""
  });

  // Per-field touched flag. A field surfaces its format error only after
  // the user has interacted with it (blur or change), matching the donor's
  // `markTouched` pattern.
  let touched = $state<TouchedMap>({});
  let submitting = $state<boolean>(false);

  // "How to fill this in" help dialog. Local to the page — the modal is
  // purely presentational (see KycHelpModal).
  let helpOpen = $state<boolean>(false);

  // Dev-only mock affordance. Mirrors the Wallet page pattern: only render
  // dev controls when the wallet client is in mock mode. Typed locally so
  // we don't widen the global `ImportMetaEnv` type just for this flag.
  const isMockMode =
    (import.meta.env as Record<string, string | undefined>)
      .VITE_USE_MOCK_WALLET === "true";

  // Keep the calling-code synced to the selected country. Picking / changing
  // a country always resets the calling code to that country's default and
  // clears the prior state value (switching in/out of US invalidates state).
  let lastCountry = $state<string>(form.country);
  $effect(() => {
    if (form.country !== lastCountry) {
      const next = COUNTRIES.find((c) => c.code === form.country);
      if (next) {
        form.callingCode = next.callingCode;
        form.countryCallingCode = next.code;
      } else {
        form.callingCode = "";
        form.countryCallingCode = "";
      }
      // Donor parity (kyc-form-view.tsx ~L137): switching in/out of US
      // invalidates the prior state value because the validation mode
      // changes (US picker vs free-text).
      const wasUS = lastCountry === "US";
      const nowUS = form.country === "US";
      if (wasUS !== nowUS) {
        form.state = "";
      }
      lastCountry = form.country;
    }
  });

  let isUS = $derived(form.country === "US");

  // Project the wire payload into the validator's shape. `kycValidation`
  // expects digits-only callingCode (e.g. "1"); the form holds it in the
  // "+1" UX format, so we strip the leading "+" here. (Documented concession
  // — donor sends raw digits because its picker yields "1"; our select sets
  // "+1" so the on-screen Code field reads naturally.)
  function toValidationShape(p: CreateCardHolderParams): KycFormShape {
    return {
      firstName: p.firstName,
      lastName: p.lastName,
      dob: p.dob,
      email: p.email,
      country: p.country,
      callingCode: (p.callingCode ?? "").replace(/^\+/, ""),
      phoneNum: p.phoneNum,
      address: p.address,
      city: p.city,
      state: p.state,
      zipCode: p.zipCode
    };
  }

  // Required-field list for the "is the form complete" gate. Mirrors
  // FIELDS_BEFORE_PHONE + country/phone + FIELDS_AFTER_PHONE + state from
  // the donor (kyc-form-view.tsx L55-L66 and the inline country/phone/state
  // sections). State is required for every country (US picker / free text).
  const REQUIRED_KEYS: KycFieldKey[] = [
    "firstName",
    "lastName",
    "dob",
    "email",
    "country",
    "callingCode",
    "phoneNum",
    "address",
    "city",
    "state",
    "zipCode"
  ];

  // Whole-form validator output (memoised via $derived). Used both to know
  // whether the form is *valid enough* to submit, and to look up the
  // per-field error message for the currently-displayed inputs.
  let validationResult = $derived(validateForm(toValidationShape(form)));
  let clientErrors = $derived(validationResult.errors);

  // Read a form field by KycFieldKey. KycFieldKey is a subset of
  // CreateCardHolderParams keys, so direct indexing through the bridge
  // shape (KycFormShape) keeps this type-safe.
  function readField(key: KycFieldKey): string {
    const shape = toValidationShape(form);
    return shape[key] ?? "";
  }

  // isComplete: every required field has a non-empty value AND the
  // validator finds no format errors. The Submit button is disabled until
  // both conditions hold, which is how the user is told "you can't submit
  // yet" — instead of plastering "Required" under every empty input.
  let isComplete = $derived(
    REQUIRED_KEYS.every((k) => readField(k).trim().length > 0) &&
      Object.keys(clientErrors).length === 0
  );

  // Error displayed for a field: only when the user has touched it AND
  // the field has content AND the validator returned an error. Empty
  // + untouched = silent (the disabled Submit button is the signal).
  function errorFor(key: KycFieldKey): string | undefined {
    if (!touched[key]) return undefined;
    const v = readField(key).trim();
    if (!v) return undefined;
    return clientErrors[key];
  }

  function markTouched(key: KycFieldKey): void {
    if (!touched[key]) touched = { ...touched, [key]: true };
  }

  // Dev affordance: populate every field with a valid sample payload and
  // mark each as touched so blur-driven UI (errors / derived state) settles
  // as if the user had typed it. Does NOT auto-submit — the user reviews
  // and clicks Submit themselves.
  function prefillMock(): void {
    form = {
      firstName: "Yuna",
      lastName: "Tanaka",
      dob: "1995-04-15",
      email: "yuna@worldagent.ai",
      callingCode: "+1",
      countryCallingCode: "1",
      phoneNum: "4159876543",
      cellNum: "",
      address: "123 Market Street",
      city: "San Francisco",
      state: "CA",
      country: "US",
      zipCode: "94103"
    };
    // Keep the country-change effect from clobbering the calling code /
    // state when `country` flips from "" to "US": align the watcher's
    // previous-country snapshot to the new value so the effect is a no-op.
    lastCountry = "US";
    touched = {
      firstName: true,
      lastName: true,
      dob: true,
      email: true,
      country: true,
      callingCode: true,
      phoneNum: true,
      address: true,
      city: true,
      state: true,
      zipCode: true
    };
  }

  async function handleSubmit(event: Event): Promise<void> {
    event.preventDefault();
    if (submitting) return;
    // The button is gated by `isComplete`; defence-in-depth re-check in
    // case some path triggers submit (e.g. Enter key) when not ready.
    if (!isComplete) return;
    submitting = true;
    try {
      // cellNum mirrors phoneNum on submit — the contract has both for
      // legacy reasons; the spec treats them as the same value.
      const payload: CreateCardHolderParams = { ...form, cellNum: form.phoneNum };
      const result = await walletClient.startKyc(payload);
      walletState.ekycUrl = result.ekycUrl;
      navigate("/wallet/kyc/verify");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      pushToast(`KYC failed: ${msg}`, "error");
    } finally {
      submitting = false;
    }
  }

  function handleCancel(): void {
    navigate("/wallet/kyc");
  }
</script>

<section class="kyc-form">
  <header class="kyc-form__head">
    <div class="kyc-form__title-row">
      <h1 class="text-section">Identity verification</h1>
      <div class="kyc-form__actions">
        <button
          type="button"
          class="help-link"
          onclick={() => (helpOpen = true)}
          aria-haspopup="dialog"
        >
          <Info size={14} strokeWidth={1.75} aria-hidden="true" />
          How to fill this in
        </button>
        {#if isMockMode}
          <button
            type="button"
            class="dev-link"
            onclick={prefillMock}
            aria-label="Prefill form with mock data (dev only)"
          >
            Prefill (mock)
          </button>
        {/if}
      </div>
    </div>
    <p class="kyc-form__intro">
      We need a few details to verify your identity before activating your card.
    </p>
  </header>

  <form class="kyc-form__form" onsubmit={handleSubmit} novalidate>
    <div class="row row--two">
      <label class="field">
        <span class="field__label">First name</span>
        <input
          class="field__input"
          class:field__input--err={errorFor("firstName")}
          type="text"
          autocomplete="given-name"
          bind:value={form.firstName}
          onblur={() => markTouched("firstName")}
        />
        {#if errorFor("firstName")}<span class="field__err">{errorFor("firstName")}</span>{/if}
      </label>
      <label class="field">
        <span class="field__label">Last name</span>
        <input
          class="field__input"
          class:field__input--err={errorFor("lastName")}
          type="text"
          autocomplete="family-name"
          bind:value={form.lastName}
          onblur={() => markTouched("lastName")}
        />
        {#if errorFor("lastName")}<span class="field__err">{errorFor("lastName")}</span>{/if}
      </label>
    </div>

    <div class="row row--two">
      <label class="field">
        <span class="field__label">Date of birth</span>
        <input
          class="field__input"
          class:field__input--err={errorFor("dob")}
          type="date"
          bind:value={form.dob}
          onblur={() => markTouched("dob")}
        />
        {#if errorFor("dob")}<span class="field__err">{errorFor("dob")}</span>{/if}
      </label>
      <label class="field">
        <span class="field__label">Email</span>
        <input
          class="field__input"
          class:field__input--err={errorFor("email")}
          type="email"
          autocomplete="email"
          bind:value={form.email}
          onblur={() => markTouched("email")}
        />
        {#if errorFor("email")}<span class="field__err">{errorFor("email")}</span>{/if}
      </label>
    </div>

    <label class="field">
      <span class="field__label">Country</span>
      <select
        class="field__input field__input--select"
        class:field__input--err={errorFor("country")}
        bind:value={form.country}
        onblur={() => markTouched("country")}
        onchange={() => markTouched("country")}
      >
        <option value="" disabled>Select a country…</option>
        {#each COUNTRIES as c (c.code)}
          <option value={c.code}>{c.name} ({c.code})</option>
        {/each}
      </select>
      {#if errorFor("country")}<span class="field__err">{errorFor("country")}</span>{/if}
    </label>

    <div class="row row--phone">
      <label class="field field--code">
        <span class="field__label">Code</span>
        <input
          class="field__input"
          class:field__input--err={errorFor("callingCode")}
          type="text"
          bind:value={form.callingCode}
          onblur={() => markTouched("callingCode")}
        />
        {#if errorFor("callingCode")}<span class="field__err">{errorFor("callingCode")}</span>{/if}
      </label>
      <label class="field field--phone">
        <span class="field__label">Phone number</span>
        <input
          class="field__input"
          class:field__input--err={errorFor("phoneNum")}
          type="tel"
          autocomplete="tel-national"
          bind:value={form.phoneNum}
          onblur={() => markTouched("phoneNum")}
        />
        {#if errorFor("phoneNum")}<span class="field__err">{errorFor("phoneNum")}</span>{/if}
      </label>
    </div>

    <label class="field">
      <span class="field__label">Address</span>
      <input
        class="field__input"
        class:field__input--err={errorFor("address")}
        type="text"
        autocomplete="street-address"
        bind:value={form.address}
        onblur={() => markTouched("address")}
      />
      {#if errorFor("address")}<span class="field__err">{errorFor("address")}</span>{/if}
    </label>

    <div class="row row--three">
      <label class="field">
        <span class="field__label">City</span>
        <input
          class="field__input"
          class:field__input--err={errorFor("city")}
          type="text"
          autocomplete="address-level2"
          bind:value={form.city}
          onblur={() => markTouched("city")}
        />
        {#if errorFor("city")}<span class="field__err">{errorFor("city")}</span>{/if}
      </label>
      <label class="field">
        <span class="field__label">State / Region</span>
        {#if isUS}
          <select
            class="field__input field__input--select"
            class:field__input--err={errorFor("state")}
            bind:value={form.state}
            onblur={() => markTouched("state")}
            onchange={() => markTouched("state")}
          >
            <option value="" disabled>—</option>
            {#each US_STATES as st (st)}
              <option value={st}>{st}</option>
            {/each}
          </select>
        {:else}
          <input
            class="field__input"
            class:field__input--err={errorFor("state")}
            type="text"
            autocomplete="address-level1"
            bind:value={form.state}
            onblur={() => markTouched("state")}
          />
        {/if}
        {#if errorFor("state")}<span class="field__err">{errorFor("state")}</span>{/if}
      </label>
      <label class="field">
        <span class="field__label">Zip / Postal</span>
        <input
          class="field__input"
          class:field__input--err={errorFor("zipCode")}
          type="text"
          autocomplete="postal-code"
          bind:value={form.zipCode}
          onblur={() => markTouched("zipCode")}
        />
        {#if errorFor("zipCode")}<span class="field__err">{errorFor("zipCode")}</span>{/if}
      </label>
    </div>

    <footer class="kyc-form__footer">
      <Button
        variant="secondary"
        size="md"
        label="Back"
        onclick={handleCancel}
        disabled={submitting}
      />
      <Button
        variant="primary"
        size="md"
        type="submit"
        label={submitting ? "Submitting…" : "Submit"}
        disabled={submitting || !isComplete}
      />
    </footer>
  </form>

  <KycHelpModal open={helpOpen} onClose={() => (helpOpen = false)} />
</section>

<style>
  .kyc-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
    padding-bottom: var(--space-7);
  }

  .kyc-form__head {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .kyc-form__title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .kyc-form__actions {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  /* Always-visible help trigger. Quiet by default (muted text + icon),
     turns into the cream accent on hover so it reads as actionable. */
  .help-link {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 2px 4px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--color-text-secondary);
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    border-radius: var(--radius-control);
    transition: background-color 120ms ease, color 120ms ease;
  }
  .help-link:hover {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
  }
  .help-link:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 1px;
  }

  /* Mirrors the dev-link style on the Wallet page so the affordance feels
     consistent across the wallet flow. Only rendered under
     VITE_USE_MOCK_WALLET=true, so it's invisible in production builds. */
  .dev-link {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 2px 4px;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    border-radius: var(--radius-control);
    transition: background-color 120ms ease, color 120ms ease;
  }
  .dev-link:hover {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
    text-decoration: underline;
  }

  .kyc-form__intro {
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    color: var(--color-text-secondary);
  }

  .kyc-form__form {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .row {
    display: grid;
    gap: var(--space-4);
  }
  .row--two {
    grid-template-columns: 1fr 1fr;
  }
  .row--three {
    grid-template-columns: 1.4fr 1fr 1fr;
  }
  .row--phone {
    grid-template-columns: 96px 1fr;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .field__label {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-secondary);
  }
  .field__input {
    appearance: none;
    background: var(--color-surface-input);
    border: 1px solid var(--color-input-border);
    border-radius: var(--radius-control);
    height: 36px;
    padding: 0 var(--space-3);
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    color: var(--color-text-primary);
    transition: border-color 120ms ease;
  }
  .field__input:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 1px;
    border-color: var(--color-action-cream-border);
  }
  .field__input--err {
    border-color: #dc2626;
  }
  .field__input--select {
    padding-right: var(--space-2);
  }
  .field__err {
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    color: #dc2626;
  }

  .kyc-form__footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--space-2);
    padding-top: var(--space-4);
  }
</style>
