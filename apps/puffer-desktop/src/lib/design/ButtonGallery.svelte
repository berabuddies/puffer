<script lang="ts">
  import Icon from "./Icon.svelte";
</script>

<div class="button-gallery" data-accent="default">
  <section class="gallery-section">
    <div class="section-heading">
      <p>Button</p>
      <span>Shared action primitive used across workspace, settings, deployments, and tasks.</span>
    </div>

    <div class="button-grid" aria-label="Button variants">
      <button type="button" class="sc-btn" data-variant="default">
        <Icon name="plus" />
        Create project
      </button>
      <button type="button" class="sc-btn" data-variant="outline">
        <Icon name="refresh" />
        Refresh
      </button>
      <button type="button" class="sc-btn" data-variant="secondary">
        <Icon name="settings" />
        Configure
      </button>
      <button type="button" class="sc-btn" data-variant="ghost">
        <Icon name="x" />
        Cancel
      </button>
      <button type="button" class="sc-btn" data-variant="destructive">
        <Icon name="trash" />
        Delete
      </button>
      <button type="button" class="sc-btn" data-variant="outline" disabled>
        <Icon name="lock" />
        Disabled
      </button>
    </div>
  </section>

  <section class="gallery-section">
    <div class="section-heading">
      <p>Sizes</p>
      <span>Compact action densities for toolbars, forms, and icon-only controls.</span>
    </div>

    <div class="size-row" aria-label="Button sizes">
      <button type="button" class="sc-btn" data-variant="default">
        Default
      </button>
      <button type="button" class="sc-btn" data-variant="outline" data-size="sm">
        Small
      </button>
      <button type="button" class="sc-btn" data-variant="secondary" data-size="xs">
        Extra small
      </button>
      <button type="button" class="sc-btn" data-variant="outline" data-size="icon-sm" aria-label="Search">
        <Icon name="search" />
      </button>
      <button type="button" class="sc-btn" data-variant="ghost" data-size="icon-sm" aria-label="More options">
        <Icon name="moreH" />
      </button>
    </div>
  </section>

  <section class="gallery-section">
    <div class="section-heading">
      <p>Composer</p>
      <span>Prompt send, stop, and disabled states from the agent conversation flow.</span>
    </div>

    <div class="composer-row" aria-label="Composer buttons">
      <button type="button" class="pf-send-btn" aria-label="Send">
        <Icon name="arrowUp" />
      </button>
      <button type="button" class="pf-send-btn pf-stop-preview" aria-label="Stop">
        <Icon name="pause2" />
      </button>
      <button type="button" class="pf-send-btn" aria-label="Send disabled" disabled>
        <Icon name="arrowUp" />
      </button>
    </div>
  </section>

  <section class="gallery-section">
    <div class="section-heading">
      <p>Modal Controls</p>
      <span>Close buttons and segmented controls used by workspace modals.</span>
    </div>

    <div class="modal-strip" aria-label="Modal buttons">
      <button type="button" class="pf-modal-close" aria-label="Close">
        <Icon name="x" />
      </button>

      <div class="segment-group" role="group" aria-label="Workspace mode">
        <button type="button" class="pf-modal-seg-btn" data-active="true">
          <Icon name="folder" />
          Recent
        </button>
        <button type="button" class="pf-modal-seg-btn">
          <Icon name="git" />
          Git
        </button>
        <button type="button" class="pf-modal-seg-btn">
          <Icon name="plus" />
          New
        </button>
      </div>
    </div>
  </section>
</div>

<style>
  .button-gallery {
    min-height: 100vh;
    padding: 40px;
    background: var(--background);
    color: var(--foreground);
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .gallery-section {
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--card);
    box-shadow: var(--shadow-xs);
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }

  .section-heading {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 18px;
  }

  .section-heading p {
    margin: 0;
    font-size: 14px;
    font-weight: 650;
  }

  .section-heading span {
    color: var(--muted-foreground);
    font-size: 12px;
  }

  .button-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }

  .size-row,
  .composer-row,
  .modal-strip,
  .segment-group {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 12px;
  }

  .composer-row {
    background: var(--muted);
    border: 1px solid var(--border);
    border-radius: 16px;
    padding: 12px;
    width: fit-content;
  }

  .pf-stop-preview {
    background: color-mix(in oklab, var(--destructive) 16%, var(--background));
    color: var(--destructive);
  }

  .segment-group {
    gap: 8px;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: 14px;
    background: var(--muted);
  }
</style>
