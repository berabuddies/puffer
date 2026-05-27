<script lang="ts">
  import Button from "../components/common/Button.svelte";
  import { navigate } from "../router.svelte";
</script>

<section class="not-found">
  <h1 class="text-display">Page not found</h1>
  <p class="text-subtitle">The route you tried doesn't match any page.</p>
  <div class="not-found__action">
    <Button variant="primary" size="md" label="Back to Home" onclick={() => navigate("/home")} />
  </div>
</section>

<style>
  .not-found {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    text-align: center;
    padding: var(--space-7) var(--space-5);
  }

  .not-found__action {
    margin-top: var(--space-4);
  }
</style>
