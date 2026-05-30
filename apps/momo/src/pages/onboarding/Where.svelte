<!--
  Onboarding · Where (artboard 23H-0)

  Mascot + greeting + a wrapped two-line subtitle + 5 country chips. The
  artboard ships a hidden "Next" button — we resolve that as auto-advance
  300ms after a chip is clicked so the user sees their selection register.
-->
<script lang="ts">
  import Chip from "../../components/common/Chip.svelte";
  import OnboardingShell from "../../components/onboarding/OnboardingShell.svelte";
  import { navigate } from "../../router.svelte";
  import { pushToast } from "../../lib/toast.svelte";
  import { setCountry } from "../../lib/onboarding.svelte";

  // Default selection per design (United States is filled black).
  let selected = $state<string>("United States");

  const countries = ["United States", "China", "Japan", "Korea", "Singapore"];
  let advancing = $state<boolean>(false);

  function pick(country: string): void {
    if (advancing) return;
    selected = country;
    setCountry(country);
    advancing = true;
    pushToast(`Where: ${country}`, "info");
    // Brief pause so the chip's selected state visibly registers before
    // the route changes. 300ms matches the spec note.
    window.setTimeout(() => navigate("/onboarding/role"), 300);
  }
</script>

<OnboardingShell
  title="Hi Yuna. I'm Momo"
  bodyLines={["Let's get to know you a little better.", "Like, where you live?"]}
  skipTo="/home"
>
  <div class="chips-row">
    {#each countries as country (country)}
      <Chip
        variant="select"
        label={country}
        selected={selected === country}
        onclick={() => pick(country)}
      />
    {/each}
  </div>
</OnboardingShell>

<style>
  .chips-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    /* Profession grid = 16px gap per Paper. */
    gap: 16px;
  }
</style>
