/**
 * Wallet UI bug-surface spec.
 *
 * The wallet UI is brand-new and the user has already flagged sloppy
 * interactions just by clicking around. This spec systematically pokes
 * every state, modal, and form path through the in-page mock (driven by
 * VITE_USE_MOCK_WALLET=true → walletClient.svelte.ts → window.__wallet)
 * so the failing assertions surface real defects in the UI layer.
 *
 * Important: we do NOT fix the wallet UI here. Failures are the deliverable.
 * If a test is genuinely environmental (timing, etc.) it gets `test.skip`
 * with a TODO. Anything else is allowed to fail and is reported.
 */

import { expect, test, type Page, type ConsoleMessage } from "@playwright/test";
import type { KycStatus } from "../src/lib/walletTypes";

// Shape of window.__wallet exposed by the mock. Kept in sync manually with
// walletClient.svelte.ts; if you add a knob there add it here.
type WalletKnobs = {
  // Simulated backend events — the real driver of the state machine.
  simulateApproveKyc(): void;
  simulateDeclineKyc(opts?: { admin?: boolean }): void;
  simulateDeposit(amountUsd: number, opts?: { from?: string; label?: string }): void;
  simulateSpend(amountUsd: number, opts?: { merchant?: string; description?: string }): void;
  reset(): void;
  getState(): Readonly<Record<string, unknown>>;
  // Mock knobs.
  setMockFailure(method: string | null, message?: string): void;
  setMockLatency(ms: number | null): void;
  getCallCount(method: string): number;
  resetMockState(): void;
  MOCK_DEPOSIT_ADDRESS: string;
  // Legacy shortcuts — prefer simulate* APIs above.
  setMockKycStatus(s: KycStatus): void;
  setMockHasFunds(v: boolean): void;
};

declare global {
  interface Window {
    __wallet?: WalletKnobs;
  }
}

/**
 * Sign-in bootstrap — mirrors chat.spec.ts `bootOnboarded` but skips the
 * FakeDaemon wiring (wallet uses REST mocks, not WS). Stubs an unexpired
 * Auth Station-shaped JWT into localStorage so `loadAuthFromStorage` flips
 * authState.status to signedIn before any route renders.
 */
async function bootSignedIn(page: Page): Promise<void> {
  await page.addInitScript(() => {
    try {
      window.localStorage.setItem("puffer.onboarded", "true");
      const header = btoa(JSON.stringify({ alg: "RS256", typ: "JWT" }))
        .replace(/=+$/, "")
        .replace(/\+/g, "-")
        .replace(/\//g, "_");
      const payload = btoa(
        JSON.stringify({
          sub: "test-user",
          email: "test@example.com",
          name: "Test User",
          exp: Math.floor(Date.now() / 1000) + 60 * 60 * 24
        })
      )
        .replace(/=+$/, "")
        .replace(/\+/g, "-")
        .replace(/\//g, "_");
      window.localStorage.setItem(
        "puffer.authToken",
        `${header}.${payload}.test-sig`
      );
    } catch {
      /* private mode — auth.svelte.ts treats absence as not-onboarded. */
    }
  });
}

/** Set kyc status + funds via the page-exposed knobs. Must be called AFTER
 *  navigation (because window.__wallet is hung off the live module). */
async function setStatus(
  page: Page,
  status: KycStatus,
  hasFunds: boolean = false
): Promise<void> {
  await page.evaluate(
    ([s, f]) => {
      window.__wallet?.setMockKycStatus(s as KycStatus);
      window.__wallet?.setMockHasFunds(f as boolean);
    },
    [status, hasFunds] as const
  );
}

async function resetMock(page: Page): Promise<void> {
  await page.evaluate(() => window.__wallet?.resetMockState());
}

/** Helper: collect all `console.error` messages during a block so tests
 *  can assert "no uncaught errors". */
function trapConsoleErrors(page: Page): { errors: string[]; stop: () => void } {
  const errors: string[] = [];
  const handler = (msg: ConsoleMessage): void => {
    if (msg.type() === "error") errors.push(msg.text());
  };
  page.on("console", handler);
  return {
    errors,
    stop: () => page.off("console", handler)
  };
}

test.beforeEach(async ({ page }) => {
  await bootSignedIn(page);
  // Boot once on /home so the mock module is loaded + window.__wallet exists.
  // Then reset state and we're ready for each test to drive its own scenario.
  await page.goto("/#/home");
  await page.waitForFunction(() => !!window.__wallet, { timeout: 10_000 });
  await resetMock(page);
});

/* ─────────────────────────────────────────────────────────────────────
   A. Routing + status branching
   ───────────────────────────────────────────────────────────────────── */

test.describe("A. Routing & status branching", () => {
  test("A1: /wallet with status=none redirects to /wallet/kyc", async ({ page }) => {
    await setStatus(page, "none");
    await page.goto("/#/wallet");
    await expect(page).toHaveURL(/#\/wallet\/kyc$/);
  });

  test("A2: /wallet with status=in_review stays on /wallet and shows 'In Review' KPI", async ({
    page
  }) => {
    await setStatus(page, "in_review");
    await page.goto("/#/wallet");
    await expect(page).toHaveURL(/#\/wallet$/);
    await expect(page.getByRole("heading", { name: "Wallet" })).toBeVisible();
    await expect(page.getByText("In Review", { exact: true })).toBeVisible();
  });

  test("A3: status=approved + no funds shows Verified, $0.00, No activity", async ({
    page
  }) => {
    await setStatus(page, "approved", false);
    await page.goto("/#/wallet");
    await expect(page.getByText("Verified", { exact: true })).toBeVisible();
    await expect(page.getByText("$0.00", { exact: true }).first()).toBeVisible();
    await expect(page.getByText("No activity")).toBeVisible();
  });

  test("A4: status=approved + funds=true renders balance + ≥3 transaction rows", async ({
    page
  }) => {
    await setStatus(page, "approved", true);
    await page.goto("/#/wallet");
    // Legacy setMockHasFunds(true) seeds a $2,418.32 deposit minus two
    // spends ($128.50 + $42.00) → net $2,247.82.
    await expect(page.getByText("$2,247.82")).toBeVisible();
    const rows = page.locator(".activity-btn");
    await expect(rows.first()).toBeVisible();
    expect(await rows.count()).toBeGreaterThanOrEqual(3);
  });

  test("A5: direct nav to /wallet/kyc/form works regardless of status", async ({
    page
  }) => {
    await setStatus(page, "approved", true);
    await page.goto("/#/wallet/kyc/form");
    await expect(page).toHaveURL(/#\/wallet\/kyc\/form$/);
    await expect(
      page.getByRole("heading", { name: "Identity verification" })
    ).toBeVisible();
  });

  test("A6: direct nav to /wallet/kyc/verify with no ekycUrl handles gracefully", async ({
    page
  }) => {
    const trap = trapConsoleErrors(page);
    await page.goto("/#/wallet/kyc/verify");
    // Either bounces back to /wallet/kyc OR renders an empty graceful state —
    // both are acceptable, but a crash / console error is not.
    await page.waitForTimeout(500);
    const url = page.url();
    const onKycEntry = /#\/wallet\/kyc$/.test(url);
    const onVerify = /#\/wallet\/kyc\/verify$/.test(url);
    expect(onKycEntry || onVerify).toBe(true);
    trap.stop();
    // No uncaught script errors. (Ignore WS-reconnect noise — chat client
    // tries to connect at module load and the FakeDaemon isn't running for
    // wallet-only specs.)
    const fatal = trap.errors.filter(
      (e) =>
        !/favicon|MockServiceWorker|DevTools/i.test(e) &&
        !/WebSocket connection to 'ws:\/\/127\.0\.0\.1:17777/i.test(e)
    );
    expect(fatal, `console errors:\n${fatal.join("\n")}`).toEqual([]);
  });
});

/* ─────────────────────────────────────────────────────────────────────
   B. Status spectrum → "In Review" privacy mapping
   ───────────────────────────────────────────────────────────────────── */

test.describe("B. Status spectrum maps to 'In Review' (no error leakage)", () => {
  const NON_APPROVED: KycStatus[] = [
    "pending",
    "draft",
    "in_review",
    "declined",
    "error",
    "admin_decline"
  ];

  for (const status of NON_APPROVED) {
    test(`maps ${status} → In Review KPI; never leaks raw status / error words`, async ({
      page
    }) => {
      await setStatus(page, status);
      await page.goto("/#/wallet");
      // The Identity verification KPI must read "In Review" verbatim.
      await expect(page.getByText("In Review", { exact: true })).toBeVisible();
      // Privacy guards: none of these leaky tokens should ever surface on
      // the wallet page.
      const leaky = [
        "declined",
        "Declined",
        "DECLINED",
        "admin_decline",
        "Admin",
        "error",
        "Error",
        "draft",
        "pending",
        "Pending",
        "Identity verification declined"
      ];
      // Some words (e.g. "error") appear in legitimate places; we scope the
      // check to the Identity KPI tile only.
      const kpi = page
        .locator("button.kpi-btn", { hasText: "Identity verification" })
        .first();
      const kpiText = (await kpi.textContent()) ?? "";
      for (const word of leaky) {
        if (word === "Identity verification") continue; // benign label
        expect(
          kpiText,
          `KPI text leaks "${word}" for status=${status}: ${kpiText}`
        ).not.toContain(word);
      }
    });
  }
});

/* ─────────────────────────────────────────────────────────────────────
   C. KYC form validation
   ───────────────────────────────────────────────────────────────────── */

test.describe("C. KYC form validation", () => {
  test.beforeEach(async ({ page }) => {
    await setStatus(page, "none");
    await page.goto("/#/wallet/kyc/form");
    await expect(
      page.getByRole("heading", { name: "Identity verification" })
    ).toBeVisible();
  });

  test("C1: empty form → Submit button is disabled (no 'Required' errors)", async ({
    page
  }) => {
    // The form starts empty; the user has not interacted with any field. Per
    // the UX rule, Submit must be disabled (the only signal that more input
    // is needed) and no .field__err elements should be rendered.
    const submit = page.getByRole("button", { name: "Submit" });
    await expect(submit).toBeDisabled();
    await expect(page.locator(".field__err")).toHaveCount(0);
    await expect(page).toHaveURL(/#\/wallet\/kyc\/form$/);
  });

  test("C2: bad email format → email-specific error appears after blur", async ({
    page
  }) => {
    const email = page.getByLabel("Email");
    await email.fill("foo");
    // Blur to mark the field touched — error surfaces only after touch.
    await email.blur();
    const emailField = page.locator("label.field", { hasText: "Email" });
    await expect(emailField.locator(".field__err")).toBeVisible();
    await expect(emailField.locator(".field__err")).toContainText(
      /Invalid email format/i
    );
  });

  test("C3: DOB under 18 AND DOB in the future each produce a DOB error", async ({
    page
  }) => {
    const dobField = page.locator("label.field", { hasText: "Date of birth" });
    const dob = page.getByLabel("Date of birth");

    // Under-18 DOB. Fill + blur, then assert the age-specific message.
    await dob.fill("2020-01-01");
    await dob.blur();
    await expect(dobField.locator(".field__err")).toBeVisible();
    await expect(dobField.locator(".field__err")).toContainText(
      /at least 18 years old/i
    );

    // Future DOB — validator's isRealDate clamps to now() so dates after
    // today fail "That date does not exist."
    await dob.fill("3000-01-01");
    await dob.blur();
    await expect(dobField.locator(".field__err")).toBeVisible();
    await expect(dobField.locator(".field__err")).toContainText(
      /does not exist|double-check/i
    );
  });

  test("C4: non-digit phone → phone-specific error after blur", async ({
    page
  }) => {
    const phone = page.getByLabel("Phone number");
    await phone.fill("abc-def");
    await phone.blur();
    const phoneField = page.locator("label.field", { hasText: "Phone number" });
    await expect(phoneField.locator(".field__err")).toBeVisible();
    await expect(phoneField.locator(".field__err")).toContainText(/digit/i);
  });

  test("C5: country=US, all-but-state filled → Submit stays disabled", async ({
    page
  }) => {
    // Required-empty (here: state) keeps the Submit button gated. We are
    // explicitly NOT showing a "Required" text under the input.
    await page.getByLabel("Country").selectOption("US");
    await page.getByLabel("First name").fill("Ada");
    await page.getByLabel("Last name").fill("Lovelace");
    await page.getByLabel("Date of birth").fill("1990-05-21");
    await page.getByLabel("Email").fill("ada@example.com");
    await page.getByLabel("Phone number").fill("4155551212");
    await page.getByLabel("Address").fill("1 Infinite Loop");
    await page.getByLabel("City").fill("Cupertino");
    await page.getByLabel("Zip / Postal").fill("95014");

    const submit = page.getByRole("button", { name: "Submit" });
    await expect(submit).toBeDisabled();
    // And no "Required"-style error has appeared under the empty state field.
    const stateField = page.locator("label.field", {
      hasText: "State / Region"
    });
    await expect(stateField.locator(".field__err")).toHaveCount(0);
  });

  test("C6: changing country updates calling-code prefix", async ({ page }) => {
    const callingCode = page.getByLabel("Code");
    // Form starts empty (no country pre-selected), so calling code is blank.
    await expect(callingCode).toHaveValue("");
    await page.getByLabel("Country").selectOption("US");
    await expect(callingCode).toHaveValue("+1");
    await page.getByLabel("Country").selectOption("GB");
    await expect(callingCode).toHaveValue("+44");
    await page.getByLabel("Country").selectOption("JP");
    await expect(callingCode).toHaveValue("+81");
  });

  test("C7: double-submit fires startKyc at most once", async ({ page }) => {
    // Fill a complete valid form so the Submit button enables.
    await page.getByLabel("Country").selectOption("US");
    await page.getByLabel("First name").fill("Ada");
    await page.getByLabel("Last name").fill("Lovelace");
    await page.getByLabel("Date of birth").fill("1990-05-21");
    await page.getByLabel("Email").fill("ada@example.com");
    await page.getByLabel("Phone number").fill("4155551212");
    await page.getByLabel("Address").fill("1 Infinite Loop");
    await page.getByLabel("City").fill("Cupertino");
    await page.getByLabel("State / Region").selectOption("CA");
    await page.getByLabel("Zip / Postal").fill("95014");

    // Bump mock latency so the second click can race the first request.
    await page.evaluate(() => window.__wallet?.setMockLatency(400));

    const submit = page.getByRole("button", { name: /Submit|Submitting/ });
    await expect(submit).toBeEnabled();
    await submit.click();
    // Try to click again immediately. The button SHOULD be disabled mid-flight.
    await submit.click({ force: true }).catch(() => {
      /* expected if the button is disabled. */
    });

    // Wait for the request to settle (navigate to verify or back to form).
    await page.waitForLoadState("networkidle").catch(() => undefined);
    await page.waitForTimeout(800);

    const calls = await page.evaluate(() =>
      window.__wallet?.getCallCount("startKyc")
    );
    expect(calls, "startKyc must only fire once on double-submit").toBeLessThanOrEqual(1);
  });

  test("C8: valid submit transitions to /wallet/kyc/verify", async ({ page }) => {
    await page.getByLabel("Country").selectOption("US");
    await page.getByLabel("First name").fill("Ada");
    await page.getByLabel("Last name").fill("Lovelace");
    await page.getByLabel("Date of birth").fill("1990-05-21");
    await page.getByLabel("Email").fill("ada@example.com");
    await page.getByLabel("Phone number").fill("4155551212");
    await page.getByLabel("Address").fill("1 Infinite Loop");
    await page.getByLabel("City").fill("Cupertino");
    await page.getByLabel("State / Region").selectOption("CA");
    await page.getByLabel("Zip / Postal").fill("95014");

    await page.getByRole("button", { name: "Submit" }).click();
    await expect(page).toHaveURL(/#\/wallet\/kyc\/verify$/, { timeout: 5000 });
  });

  test("C9: email over 36 characters → length-specific error", async ({
    page
  }) => {
    // TC37 one-pager caps the email at 36 chars (was 50). 40-char address.
    const email = page.getByLabel("Email");
    await email.fill("aaaaaaaaaaaaaaaaaaaaaaaaaaaa@example.com");
    await email.blur();
    const emailField = page.locator("label.field", { hasText: "Email" });
    await expect(emailField.locator(".field__err")).toContainText(
      /Maximum 36 characters/i
    );
  });

  test('C10: email containing "+" → plus-specific error', async ({ page }) => {
    // Strada rejects "+" in the email; we surface a specific message rather
    // than the generic "Invalid email format".
    const email = page.getByLabel("Email");
    await email.fill("ada+test@example.com");
    await email.blur();
    const emailField = page.locator("label.field", { hasText: "Email" });
    await expect(emailField.locator(".field__err")).toContainText(
      /cannot contain a "\+"/i
    );
  });

  test("C11: help trigger opens the field-guidance modal; ESC + close dismiss it", async ({
    page
  }) => {
    const dialog = page.getByRole("dialog", {
      name: "How to fill in your details"
    });
    await expect(dialog).toBeHidden();

    // Open via the always-visible header link.
    await page.getByRole("button", { name: "How to fill this in" }).click();
    await expect(dialog).toBeVisible();
    // A representative PDF-sourced rule is present.
    await expect(dialog).toContainText(/Max 36 characters/i);

    // ESC dismisses (Modal wires window keydown).
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();

    // Re-open and dismiss via the close button.
    await page.getByRole("button", { name: "How to fill this in" }).click();
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toBeHidden();
  });
});

/* ─────────────────────────────────────────────────────────────────────
   D. Top-up modal interactions
   ───────────────────────────────────────────────────────────────────── */

test.describe("D. Top-up modal", () => {
  test.beforeEach(async ({ page }) => {
    await setStatus(page, "approved", true);
    await page.goto("/#/wallet");
    await expect(page.getByText("$2,247.82")).toBeVisible();
  });

  async function openModal(page: Page): Promise<void> {
    await page.getByRole("button", { name: /Top up/ }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
  }

  test("D1: clicking Top up opens modal with title + subtitle", async ({
    page
  }) => {
    await openModal(page);
    await expect(
      page.getByRole("heading", { name: "Top up", exact: true })
    ).toBeVisible();
    await expect(page.getByText(/Arrives ~1 min/)).toBeVisible();
  });

  test("D2: QR image is rendered inside the modal", async ({ page }) => {
    await openModal(page);
    const qr = page.locator(
      ".modal-panel img[src^='data:image'], .modal-panel canvas"
    );
    await expect(qr.first()).toBeVisible({ timeout: 5000 });
  });

  test("D3: deposit address text matches the mock's MOCK_DEPOSIT_ADDRESS", async ({
    page
  }) => {
    await openModal(page);
    const expected: string = await page.evaluate(
      () => window.__wallet?.MOCK_DEPOSIT_ADDRESS ?? ""
    );
    expect(expected.length).toBeGreaterThan(10);
    await expect(page.locator(".address-text")).toHaveText(expected);
  });

  test("D4: Copy address writes the deposit address to the clipboard", async ({
    page,
    context
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await openModal(page);
    const expected: string = await page.evaluate(
      () => window.__wallet?.MOCK_DEPOSIT_ADDRESS ?? ""
    );
    // Wait for address to land before clicking copy (button is disabled
    // until `address` resolves).
    await expect(page.locator(".address-text")).toHaveText(expected);
    await page.getByRole("button", { name: "Copy address" }).click();
    const clip = await page.evaluate(() => navigator.clipboard.readText());
    expect(clip).toBe(expected);
  });

  test("D5: ESC closes the modal", async ({ page }) => {
    await openModal(page);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toBeHidden();
  });

  test("D6: backdrop click closes the modal", async ({ page }) => {
    await openModal(page);
    // Click the backdrop at (5,5) so we land outside the centered panel.
    const backdrop = page.locator(".modal-backdrop");
    await backdrop.click({ position: { x: 5, y: 5 } });
    await expect(page.getByRole("dialog")).toBeHidden();
  });

  test("D7: clicking inside the panel does NOT close the modal", async ({
    page
  }) => {
    await openModal(page);
    // Click the modal title — a panel descendant.
    await page.getByRole("heading", { name: "Top up", exact: true }).click();
    // Give any erroneous close handler a tick to fire.
    await page.waitForTimeout(150);
    await expect(page.getByRole("dialog")).toBeVisible();
  });

  test("D8: Cancel button closes the modal", async ({ page }) => {
    await openModal(page);
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByRole("dialog")).toBeHidden();
  });

  test("D9: Deposited button closes the modal", async ({ page }) => {
    await openModal(page);
    await page.getByRole("button", { name: "Deposited" }).click();
    await expect(page.getByRole("dialog")).toBeHidden();
  });

  test("D10: closing the modal refetches balance (getBalance call count grows)", async ({
    page
  }) => {
    const before = (await page.evaluate(() =>
      window.__wallet?.getCallCount("getBalance")
    )) as number;
    await openModal(page);
    await page.getByRole("button", { name: "Deposited" }).click();
    await expect(page.getByRole("dialog")).toBeHidden();
    // Allow the refresh() async chain to settle.
    await page.waitForTimeout(800);
    const after = (await page.evaluate(() =>
      window.__wallet?.getCallCount("getBalance")
    )) as number;
    expect(after).toBeGreaterThan(before);
  });

  test("D11: body scroll is locked while open, restored after close", async ({
    page
  }) => {
    const initial = await page.evaluate(() => document.body.style.overflow);
    await openModal(page);
    const whileOpen = await page.evaluate(() => document.body.style.overflow);
    expect(whileOpen).toBe("hidden");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toBeHidden();
    const afterClose = await page.evaluate(() => document.body.style.overflow);
    expect(afterClose).toBe(initial);
  });

  test("D12: open → close → open again works (no leaked listeners, no double-modal)", async ({
    page
  }) => {
    await openModal(page);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toBeHidden();
    await openModal(page);
    // Exactly one dialog visible.
    expect(await page.getByRole("dialog").count()).toBe(1);
  });
});

/* ─────────────────────────────────────────────────────────────────────
   E. Network failure paths
   ───────────────────────────────────────────────────────────────────── */

test.describe("E. Network failure paths", () => {
  test("E1: getKycStatus rejection doesn't white-screen the wallet route", async ({
    page
  }) => {
    const trap = trapConsoleErrors(page);
    await setStatus(page, "approved");
    await page.evaluate(() =>
      window.__wallet?.setMockFailure("getKycStatus", "boom")
    );
    await page.goto("/#/wallet");
    // Some visible content must render — header or any wallet shell text.
    // (We don't care WHAT recovers; we care that something does.)
    await page.waitForTimeout(1500);
    const bodyText = (await page.locator("body").innerText()).trim();
    expect(bodyText.length, "page should not be blank on getKycStatus failure")
      .toBeGreaterThan(0);
    trap.stop();
    // Ignore: favicon noise; the configured 'boom' message we injected (it
    // SHOULD surface as a toast, not a thrown uncaught error); and the WS
    // reconnect error that the V2 chat client emits at module load when the
    // FakeDaemon isn't installed for this spec (unrelated to wallet).
    const fatal = trap.errors.filter(
      (e) =>
        !/favicon|MockServiceWorker|DevTools|boom/i.test(e) &&
        !/WebSocket connection to 'ws:\/\/127\.0\.0\.1:17777/i.test(e)
    );
    expect(
      fatal,
      `unexpected console errors: ${fatal.join("\n")}`
    ).toEqual([]);
  });

  test("E2: getBalance rejection still renders the wallet shell", async ({
    page
  }) => {
    await setStatus(page, "approved", true);
    await page.evaluate(() =>
      window.__wallet?.setMockFailure("getBalance", "balance boom")
    );
    await page.goto("/#/wallet");
    // Header must still render even if balance failed.
    await expect(page.getByRole("heading", { name: "Wallet" })).toBeVisible({
      timeout: 5000
    });
  });

  test("E3: getDepositAddress rejection doesn't crash the Top-up modal", async ({
    page
  }) => {
    await setStatus(page, "approved", true);
    await page.goto("/#/wallet");
    await expect(page.getByText("$2,247.82")).toBeVisible();
    await page.evaluate(() =>
      window.__wallet?.setMockFailure("getDepositAddress", "no address")
    );
    await page.getByRole("button", { name: /Top up/ }).click();
    // Modal must open even though the address fetch failed.
    await expect(page.getByRole("dialog")).toBeVisible();
    // Either an error placeholder or empty address — both pass; the only
    // failure mode is a crash. We assert the modal stays open after the
    // failed request settles.
    await page.waitForTimeout(800);
    await expect(page.getByRole("dialog")).toBeVisible();
  });
});

/* ─────────────────────────────────────────────────────────────────────
   F. Top-up button enablement
   ───────────────────────────────────────────────────────────────────── */

test.describe("F. Top-up button enablement vs kyc status", () => {
  test("F1: status=in_review — Top up either no-ops or opens; must not crash", async ({
    page
  }) => {
    await setStatus(page, "in_review");
    await page.goto("/#/wallet");
    await expect(page.getByText("In Review", { exact: true })).toBeVisible();

    // The Chip primitive flips to a <span> (non-button) when its onclick is
    // undefined, so `getByRole('button')` won't match in the gated case.
    // Look for the chip by text instead and click whatever element is there.
    const topup = page.locator(".chip", { hasText: "Top up" }).first();
    await topup.click({ force: true, timeout: 2000 }).catch(() => undefined);
    // Either no modal (gated) or a modal (ungated). Either is acceptable —
    // we only assert there's no thrown error or broken render.
    await page.waitForTimeout(300);
    // Heading still there → page is alive.
    await expect(page.getByRole("heading", { name: "Wallet" })).toBeVisible();
  });

  test("F2: status=approved — Top up opens the modal", async ({ page }) => {
    await setStatus(page, "approved", true);
    await page.goto("/#/wallet");
    await page.getByRole("button", { name: /Top up/ }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
  });
});

/* ─────────────────────────────────────────────────────────────────────
   G. Sidebar nav sanity
   ───────────────────────────────────────────────────────────────────── */

test.describe("G. Sidebar navigation sanity", () => {
  test("G1: Wallet sidebar entry has aria-current=page on any /wallet/* route", async ({
    page
  }) => {
    await setStatus(page, "approved", true);
    await page.goto("/#/wallet");
    const walletRow = page.locator("button.nav-row", { hasText: "Wallet" });
    await expect(walletRow).toHaveAttribute("aria-current", "page");

    await page.goto("/#/wallet/kyc/form");
    await expect(walletRow).toHaveAttribute("aria-current", "page");
  });

  test("G2: navigating Home → Wallet preserves kycStatus across in-page navigation", async ({
    page
  }) => {
    await setStatus(page, "approved", true);
    await page.goto("/#/wallet");
    await expect(page.getByText("$2,247.82")).toBeVisible();

    // Click Home in the sidebar.
    await page.locator("button.nav-row", { hasText: "Home" }).click();
    await expect(page).toHaveURL(/#\/home$/);

    // Back to Wallet.
    await page.locator("button.nav-row", { hasText: "Wallet" }).click();
    await expect(page).toHaveURL(/#\/wallet$/);
    // Status preserved → net balance still 2,247.82 (not bumped to
    // "$0.00" or bounced to /wallet/kyc).
    await expect(page.getByText("$2,247.82")).toBeVisible();
  });
});

/* ─────────────────────────────────────────────────────────────────────
   H. End-to-end state machine walk
   ───────────────────────────────────────────────────────────────────── */

test("end-to-end flow walks the state machine", async ({ page }) => {
  // Fresh boot, status=none. Visiting /wallet must bounce to /wallet/kyc.
  await page.goto("/#/wallet");
  await expect(page).toHaveURL(/#\/wallet\/kyc$/, { timeout: 5000 });

  // Click "Verify identity" → on form. The WalletKyc page exposes this
  // as the primary CTA.
  await page.getByRole("button", { name: /Verify identity/ }).click();
  await expect(page).toHaveURL(/#\/wallet\/kyc\/form$/);
  await expect(
    page.getByRole("heading", { name: "Identity verification" })
  ).toBeVisible();

  // Fill all 11 fields → Submit.
  await page.getByLabel("Country").selectOption("US");
  await page.getByLabel("First name").fill("Ada");
  await page.getByLabel("Last name").fill("Lovelace");
  await page.getByLabel("Date of birth").fill("1990-05-21");
  await page.getByLabel("Email").fill("ada@example.com");
  await page.getByLabel("Phone number").fill("4155551212");
  await page.getByLabel("Address").fill("1 Infinite Loop");
  await page.getByLabel("City").fill("Cupertino");
  await page.getByLabel("State / Region").selectOption("CA");
  await page.getByLabel("Zip / Postal").fill("95014");

  const submit = page.getByRole("button", { name: "Submit" });
  await expect(submit).toBeEnabled();
  await submit.click();

  // URL goes to /wallet/kyc/verify.
  await expect(page).toHaveURL(/#\/wallet\/kyc\/verify$/, { timeout: 5000 });

  // Simulate the eKYC webhook firing (real backend would do this when
  // the provider posts the approval result). Approving KYC does NOT issue a
  // card — the user is approved but card-less until they activate one.
  await page.evaluate(() => window.__wallet?.simulateApproveKyc());
  // Drive the bounce-home that the simulate button does for the user.
  // Calling via __wallet doesn't navigate, so do it manually to mirror
  // the click path.
  await page.goto("/#/wallet");

  // Back at /wallet: Verified badge visible, but no card yet → the Activate
  // prompt stands in for the balance row.
  await expect(page).toHaveURL(/#\/wallet$/);
  await expect(page.getByText("Verified", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Activate wallet" })
  ).toBeVisible();

  // Activate the card → issueCard fires, the snapshot reloads, and the normal
  // balance view ($0.00, no funds yet) replaces the prompt.
  await page.getByRole("button", { name: "Activate wallet" }).click();
  await expect(page.getByText("$0.00", { exact: true }).first()).toBeVisible({
    timeout: 5000
  });
  await expect(
    page.getByRole("button", { name: "Activate wallet" })
  ).toBeHidden();

  // Open Top up modal → address visible → close.
  await page.getByRole("button", { name: /Top up/ }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  const expectedAddr: string = await page.evaluate(
    () => window.__wallet?.MOCK_DEPOSIT_ADDRESS ?? ""
  );
  await expect(page.locator(".address-text")).toHaveText(expectedAddr);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeHidden();

  // Simulate an on-chain deposit, then bounce Home → Wallet to force a
  // refresh of the page's reactive state (the user would refresh the
  // page; sidebar nav has the same effect because Wallet.svelte calls
  // refresh() on mount).
  await page.evaluate(() => window.__wallet?.simulateDeposit(500));
  await page.locator("button.nav-row", { hasText: "Home" }).click();
  await expect(page).toHaveURL(/#\/home$/);
  await page.locator("button.nav-row", { hasText: "Wallet" }).click();
  await expect(page).toHaveURL(/#\/wallet$/);

  // Balance $500.00, "Crypto top up +$500.00" in activity.
  await expect(page.getByText("$500.00").first()).toBeVisible({ timeout: 5000 });
  await expect(page.getByText("Crypto top up", { exact: true })).toBeVisible();
  await expect(page.getByText("+$500.00")).toBeVisible();
});

/* ─────────────────────────────────────────────────────────────────────
   I. Card activation (approved but no card)
   ─────────────────────────────────────────────────────────────────────
   The realistic webhook path (simulateApproveKyc) leaves the user approved
   but card-less — mirroring the real backend, where a card is only created
   on POST /card/issue. The wallet must then show an explicit "Activate
   wallet" button (we never auto-issue: each user is capped at 3 cards). */

test.describe("I. Card activation", () => {
  async function bootApprovedNoCard(page: Page): Promise<void> {
    // simulateApproveKyc flips status to approved WITHOUT issuing a card.
    await page.evaluate(() => window.__wallet?.simulateApproveKyc());
    await page.goto("/#/wallet");
    await expect(page).toHaveURL(/#\/wallet$/);
  }

  test("I1: approved + no card shows the Activate prompt, not a balance", async ({
    page
  }) => {
    await bootApprovedNoCard(page);
    await expect(page.getByText("Verified", { exact: true })).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Activate your wallet" })
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Activate wallet" })
    ).toBeVisible();
    // The balance row is replaced — no Top up affordance while card-less.
    await expect(page.getByText("AVAILABLE BALANCE")).toBeHidden();
  });

  test("I2: clicking Activate issues exactly one card and reveals the balance", async ({
    page
  }) => {
    await bootApprovedNoCard(page);
    const before = (await page.evaluate(() =>
      window.__wallet?.getCallCount("issueCard")
    )) as number;

    await page.getByRole("button", { name: "Activate wallet" }).click();

    // Balance view appears ($0.00 — newly issued card has no funds).
    await expect(page.getByText("$0.00", { exact: true }).first()).toBeVisible({
      timeout: 5000
    });
    await expect(page.getByText("AVAILABLE BALANCE")).toBeVisible();
    // Activate prompt is gone.
    await expect(
      page.getByRole("button", { name: "Activate wallet" })
    ).toBeHidden();

    const after = (await page.evaluate(() =>
      window.__wallet?.getCallCount("issueCard")
    )) as number;
    expect(after - before).toBe(1);
  });

  test("I3: double-click fires issueCard at most once", async ({ page }) => {
    await bootApprovedNoCard(page);
    // Slow the mock so a second click can race the first request.
    await page.evaluate(() => window.__wallet?.setMockLatency(400));
    const btn = page.getByRole("button", { name: /Activat/ });
    await expect(btn).toBeEnabled();
    await btn.click();
    // The button disables mid-flight; a forced second click must not re-fire.
    await btn.click({ force: true }).catch(() => undefined);
    await page.waitForTimeout(900);
    const calls = (await page.evaluate(() =>
      window.__wallet?.getCallCount("issueCard")
    )) as number;
    expect(calls, "issueCard must fire at most once").toBeLessThanOrEqual(1);
  });

  test("I4: a failed issue keeps the Activate view and doesn't crash", async ({
    page
  }) => {
    await bootApprovedNoCard(page);
    await page.evaluate(() =>
      window.__wallet?.setMockFailure(
        "issueCard",
        "card limit reached: maximum 3 cards per user"
      )
    );
    await page.getByRole("button", { name: "Activate wallet" }).click();
    await page.waitForTimeout(800);
    // Still on the activate view; the button recovers to enabled for a retry.
    await expect(
      page.getByRole("button", { name: "Activate wallet" })
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Activate wallet" })
    ).toBeEnabled();
    await expect(
      page.getByRole("heading", { name: "Wallet", exact: true })
    ).toBeVisible();
  });
});
