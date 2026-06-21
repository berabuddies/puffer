<script lang="ts">
  type Props = {
    label?: string;
  };

  let { label = "Loading" }: Props = $props();
  const dots = [0, 1, 2];
</script>

<div class="pf-loading-dots" role="status" aria-label={label}>
  {#each dots as dot}
    <span class="pf-loading-dot" style={`animation-delay: ${dot * 0.15}s;`}></span>
  {/each}
</div>

<style>
  .pf-loading-dots {
    display: inline-flex;
    height: 24px;
    align-items: center;
    justify-content: center;
    gap: 4px;
  }

  .pf-loading-dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: #c1c1c1;
    box-shadow: 0 0 16px rgb(193 193 193 / 55%);
    opacity: 0.45;
    animation: pf-loading-dot-bounce 1.6s ease-in-out infinite;
  }

  @keyframes pf-loading-dot-bounce {
    0%,
    37.5%,
    100% {
      opacity: 0.45;
      transform: translateY(0);
    }

    18.75% {
      opacity: 1;
      transform: translateY(-6px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .pf-loading-dot {
      animation: none;
      opacity: 0.8;
    }
  }
</style>
