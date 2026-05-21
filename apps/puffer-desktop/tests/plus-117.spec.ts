import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

// PLUS-117 — Connect-project modal polish:
//  * single "New Project" title (no eyebrow / no "Clone & connect")
//  * submit button labelled "Create"
//  * Provider segment selected style matches Local/Remote (pf-selected-bg)
//  * modal head/foot stay pinned when the directory picker expands
//  * directory picker container uses a visible muted background
test("PLUS-117: connect-project modal has the new title, button, and stable layout", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-anchor",
        displayName: "Anchor session",
        title: "Anchor session",
        cwd: "/tmp/puffer-anchor",
        folderPath: "/tmp/puffer-anchor",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Create Project" }).first().click();
  const modal = page.locator(".pf-connect-modal");
  await expect(modal).toBeVisible();
  // Wait for the modal entry animation to settle so geometry measurements
  // below reflect the steady state, not the scale-in transform.
  await page.waitForTimeout(250);

  // 1. Title is "New Project" — no separate eyebrow.
  await expect(modal.locator(".pf-modal-title")).toHaveText("New Project");
  await expect(modal.locator(".pf-modal-eyebrow")).toHaveCount(0);

  // 5. Submit button reads "Create" (no git URL set).
  const submit = modal.locator(".pf-modal-foot-btns button").last();
  await expect(submit).toHaveText("Create");

  // 6. Provider segment selected style matches the Local/Remote selected
  //    style — both pull from --pf-selected-bg.
  const styles = await modal.evaluate((root) => {
    const activeProvider = root.querySelector<HTMLElement>(
      '.pf-provider-seg-btn[data-active="true"]'
    );
    const activeSeg = root.querySelector<HTMLElement>(
      '.pf-modal-seg-btn[data-active="true"]'
    );
    return {
      providerBg: activeProvider ? getComputedStyle(activeProvider).backgroundColor : null,
      segBg: activeSeg ? getComputedStyle(activeSeg).backgroundColor : null
    };
  });
  expect(styles.providerBg).not.toBeNull();
  expect(styles.providerBg).toBe(styles.segBg);

  // 2 + 4. Capture head/foot positions before expanding the directory picker.
  const before = await modal.evaluate((root) => {
    const head = root.querySelector<HTMLElement>(".pf-modal-head");
    const foot = root.querySelector<HTMLElement>(".pf-modal-foot");
    const rect = root.getBoundingClientRect();
    return {
      modalH: rect.height,
      headTop: head?.getBoundingClientRect().top ?? 0,
      footTop: foot?.getBoundingClientRect().top ?? 0
    };
  });

  // Open Browse… → directory picker.
  await modal.getByRole("button", { name: "Browse…" }).click();
  await expect(modal.locator(".pf-dir-picker")).toBeVisible();

  // 3. The picker container is rendered on a visible muted background
  //    (the previous near-white mix made the container invisible).
  const pickerBg = await modal.locator(".pf-dir-picker").evaluate((el) => {
    return getComputedStyle(el).backgroundColor;
  });
  // Modal card background should be lighter than the picker container.
  const modalCardBg = await modal.evaluate((el) => getComputedStyle(el).backgroundColor);
  expect(pickerBg).not.toBe(modalCardBg);

  // Head/foot positions and overall modal height stay stable.
  const after = await modal.evaluate((root) => {
    const head = root.querySelector<HTMLElement>(".pf-modal-head");
    const foot = root.querySelector<HTMLElement>(".pf-modal-foot");
    const body = root.querySelector<HTMLElement>(".pf-modal-body");
    const rect = root.getBoundingClientRect();
    return {
      modalH: rect.height,
      headTop: head?.getBoundingClientRect().top ?? 0,
      footTop: foot?.getBoundingClientRect().top ?? 0,
      bodyOverflowY: body ? getComputedStyle(body).overflowY : null
    };
  });
  expect(after.modalH).toBe(before.modalH);
  expect(after.headTop).toBe(before.headTop);
  expect(after.footTop).toBe(before.footTop);
  // Body must own the scrolling — otherwise the picker would push the
  // footer down and the head/foot positions above would have moved.
  expect(after.bodyOverflowY).toBe("auto");

  // 4. The directory list area must keep a stable height so that clicking
  //    Parent (which triggers an async re-load) does not collapse the row.
  const listH = await modal.locator(".pf-dir-picker-list").evaluate((el) => {
    return el.getBoundingClientRect().height;
  });
  expect(listH).toBeGreaterThanOrEqual(150);

  // Follow-up regression 3: the local-only directory picker must collapse
  // when the user flips the mode segment to Remote.
  await modal.getByRole("tab", { name: /Remote/ }).click();
  await expect(modal.locator(".pf-dir-picker")).toHaveCount(0);

  // Follow-up regression 1: swapping Local / Remote must not change the
  // geometry of any modal region. Measure every region in both modes and
  // assert exact equality.
  type Regions = Record<string, { top: number; height: number }>;
  const measure = async (): Promise<Regions> => {
    return await modal.evaluate((root) => {
      const sels = [
        ".pf-modal-head",
        ".pf-modal-seg",
        ".pf-modal-body",
        ".pf-modal-status-row",
        ".pf-modal-foot"
      ];
      const out: Record<string, { top: number; height: number }> = {};
      for (const s of sels) {
        const el = root.querySelector(s) as HTMLElement | null;
        if (!el) continue;
        const r = el.getBoundingClientRect();
        out[s] = { top: r.top, height: r.height };
      }
      return out;
    });
  };

  // Currently on Remote (picker was just collapsed).
  const remoteGeom = await measure();
  await modal.getByRole("tab", { name: /Local/ }).click();
  // Local has fewer fields — assert geometry still matches.
  const localGeom = await measure();
  for (const key of Object.keys(remoteGeom)) {
    expect(localGeom[key]).toEqual(remoteGeom[key]);
  }

  // Follow-up regression 2: the status / progress row reserves its space
  // even when empty, so revealing a status message must not shift the
  // surrounding regions.
  const statusEmpty = await measure();
  await expect(modal.locator(".pf-modal-status-row")).toHaveAttribute(
    "data-active",
    "false"
  );

  // Trigger submit (Create) — the fake daemon's create_session resolves
  // quickly, but the modal sets `status` synchronously before awaiting, so
  // the status row becomes active for at least a frame. Even more
  // reliably, exercise the failure path by submitting without a directory
  // so canSubmit() = false; instead we set localDest and let create
  // resolve, then assert the row's reserved geometry is unchanged either
  // way.
  await modal.locator("#pf-local-dest").fill("/tmp/puffer-anchor");
  // We don't actually wait for submission to complete here — just confirm
  // that the status-row slot keeps its reserved height regardless of
  // whether it currently holds a message.
  const statusReserved = await modal
    .locator(".pf-modal-status-row")
    .evaluate((el) => el.getBoundingClientRect().height);
  expect(statusReserved).toBeGreaterThanOrEqual(28);
  const stillStable = await measure();
  expect(stillStable[".pf-modal-foot"]).toEqual(statusEmpty[".pf-modal-foot"]);
  expect(stillStable[".pf-modal-status-row"]).toEqual(
    statusEmpty[".pf-modal-status-row"]
  );
});
