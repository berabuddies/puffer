<!--
  Onboarding · Role (artboard 26N-0)

  Mascot + greeting + single-line subtitle + 9-item role grid. The grid wraps
  to two rows. The last cell ("Type...") is a Chip-shaped text input where the
  user can type a custom role; typing into it makes that the selected role and
  clears the preset selection. Picking a preset clears the custom input.

  Like Where, the artboard's "Next" button is hidden — we auto-advance 300ms
  after a preset chip is picked. Typing a custom role advances 600ms after
  the user pauses (a debounce) to avoid jumping the page on every keystroke.
-->
<script lang="ts">
  import { onDestroy } from "svelte";

  import Chip from "../../components/common/Chip.svelte";
  import OnboardingShell from "../../components/onboarding/OnboardingShell.svelte";
  import { navigate } from "../../router.svelte";
  import { pushToast } from "../../lib/toast.svelte";

  const presetRoles = [
    "Founder",
    "Product Manager",
    "Designer",
    "Engineer",
    "Sales",
    "Student",
    "Freelancer",
    "Investor"
  ];

  // selected: matches a preset label OR the live customRole when user typed.
  let selected = $state<string>("Founder");
  let customRole = $state<string>("");
  let advancing = $state<boolean>(false);
  let customTimer: ReturnType<typeof setTimeout> | null = null;

  function pickPreset(role: string): void {
    if (advancing) return;
    selected = role;
    customRole = "";
    if (customTimer) {
      clearTimeout(customTimer);
      customTimer = null;
    }
    advancing = true;
    pushToast(`Role: ${role}`, "info");
    window.setTimeout(() => navigate("/onboarding/apps"), 300);
  }

  function onCustomInput(event: Event): void {
    const value = (event.target as HTMLInputElement).value;
    customRole = value;
    // While typing, the custom value becomes the selected role; preset chips
    // visually deselect (none of them will match this string).
    selected = value;
    if (customTimer) clearTimeout(customTimer);
    if (value.trim().length === 0) return;
    customTimer = setTimeout(() => {
      if (advancing) return;
      advancing = true;
      navigate("/onboarding/apps");
    }, 600);
  }

  function onCustomSubmit(event: SubmitEvent): void {
    event.preventDefault();
    if (advancing) return;
    if (customRole.trim().length === 0) return;
    if (customTimer) clearTimeout(customTimer);
    advancing = true;
    navigate("/onboarding/apps");
  }

  onDestroy(() => {
    if (customTimer) clearTimeout(customTimer);
  });

  // The custom chip is "selected" whenever the user has typed anything.
  let customSelected = $derived(customRole.trim().length > 0);
</script>

<OnboardingShell
  title="Hi Yuna. I'm Momo"
  subtitle="What best describes you right now?"
  skipTo="/home"
>
  <div class="chips-grid">
    {#each presetRoles as role (role)}
      <Chip
        variant="select"
        label={role}
        selected={selected === role}
        onclick={() => pickPreset(role)}
      />
    {/each}

    <form class="custom-form" onsubmit={onCustomSubmit}>
      <input
        class="custom-input"
        class:custom-input--selected={customSelected}
        type="text"
        placeholder="Type..."
        value={customRole}
        oninput={onCustomInput}
        aria-label="Type your role"
      />
    </form>
  </div>
</OnboardingShell>

<style>
  .chips-grid {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    max-width: 520px;
    gap: 16px;
  }

  /* Form wrapper so Enter submits without surrounding logic. */
  .custom-form {
    display: inline-flex;
  }

  /* Match Chip.svelte's .chip + .chip--select geometry verbatim so the
     "Type..." input reads as part of the same row. */
  .custom-input {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 6px 14px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--color-input-border);
    background: var(--color-surface-app);
    color: var(--color-text-primary);
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    /* Width sized to the placeholder ("Type...") at this font; expands
       slightly as the user types via ch units. */
    width: 84px;
    transition: background-color 120ms ease, color 120ms ease,
      border-color 120ms ease, width 120ms ease;
  }

  .custom-input::placeholder {
    color: var(--color-text-primary);
    opacity: 0.5;
  }

  .custom-input:focus {
    outline: none;
    width: 180px;
    border-color: var(--color-text-primary);
  }

  /* When the user has typed something, mirror the selected chip look so
     it integrates with the rest of the row. */
  .custom-input--selected {
    background: var(--color-text-primary);
    color: var(--color-surface-app);
    border-color: var(--color-text-primary);
    width: 180px;
  }

  .custom-input--selected::placeholder {
    color: var(--color-surface-app);
    opacity: 0.6;
  }
</style>
