import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const ALICE_AVATAR = "data:image/jpeg;base64,ZmFrZS1hdmF0YXI=";

async function openContacts(page: Page) {
  await page.locator(".pf-sidebar").getByRole("button", { name: "Contacts" }).click();
}

test("contacts tab saves a user-curated contact", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  const selectedContact = page.getByRole("complementary", { name: "Selected contact" });
  await expect(selectedContact).toContainText("Alice");
  await page.getByRole("button", { name: "Close selected contact" }).click();
  await expect(selectedContact).toBeHidden();
  await page.locator(".pf-task-row").filter({ hasText: "Alice" }).getByRole("button").first().click();
  await expect(selectedContact).toContainText("Alice");

  await page.getByRole("button", { name: "New" }).click();
  const dialog = page.getByRole("dialog", { name: "Create contact" });
  await dialog.getByLabel("Name").fill("Launch Alice");
  await dialog.getByLabel("Description").fill("Alice asks high-signal launch and support questions.");
  await dialog.getByLabel("Contact IDs").fill("telegram@alice\ngoogle@alice@example.com");
  await dialog.getByRole("button", { name: /^Create$/ }).click();

  const request = await daemon.waitForRequest("contacts_save");
  expect(request.params.contact_ids).toEqual(["google@alice@example.com", "telegram@alice"]);
  await expect(selectedContact).toContainText("Launch Alice");
});

test("contacts save selects the sanitized backend-normalized saved contact", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setContactsSnapshot({
    contacts: [
      {
        id: "contact-aaron",
        name: "Aaron",
        description: "Aaron should remain in the list but not steal selection.",
        avatar: null,
        contact_ids: ["telegram@aaron"]
      }
    ],
    candidates: []
  });
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  await page.getByRole("button", { name: "New" }).click();
  const dialog = page.getByRole("dialog", { name: "Create contact" });
  await dialog.getByLabel("Name").fill("Casey");
  await dialog.getByLabel("Description").fill("Casey has a normalized Telegram id.");
  await dialog.getByLabel("Contact IDs").fill("not-a-contact\nTelegram@@Casey\ntelegram@12345\nTelegram@@Casey");
  await dialog.getByRole("button", { name: /^Create$/ }).click();

  const request = await daemon.waitForRequest("contacts_save");
  expect(request.params.contact_ids).toEqual(["telegram@casey"]);
  const selectedContact = page.getByRole("complementary", { name: "Selected contact" });
  await expect(selectedContact).toContainText("Casey");
  await expect(selectedContact).toContainText("telegram@casey");
  await expect(selectedContact).not.toContainText("Aaron should remain");
});

test("contacts save replaces an existing saved identity", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setContactsSnapshot({
    contacts: [
      {
        id: "contact-alice",
        name: "Alice",
        description: "Original Alice record.",
        avatar: null,
        contact_ids: ["telegram@alice", "google@alice@example.com"]
      }
    ],
    candidates: []
  });
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  await page.getByRole("button", { name: "New" }).click();
  const dialog = page.getByRole("dialog", { name: "Create contact" });
  await dialog.getByLabel("Name").fill("Alice Work");
  await dialog.getByLabel("Description").fill("Replacement Alice record.");
  await dialog.getByLabel("Contact IDs").fill("Telegram@@Alice");
  await dialog.getByRole("button", { name: /^Create$/ }).click();

  const request = await daemon.waitForRequest("contacts_save");
  expect(request.params.contact_ids).toEqual(["telegram@alice"]);
  await expect(page.getByRole("heading", { name: "Contacts 1" })).toBeVisible();
  const selectedContact = page.getByRole("complementary", { name: "Selected contact" });
  await expect(selectedContact).toContainText("Alice Work");
  await expect(selectedContact).toContainText("telegram@alice");
  await expect(selectedContact).toContainText("google@alice@example.com");
  await expect(page.getByLabel("Contact list").locator(".pf-task-row")).toHaveCount(1);
});

test("contacts list lazily renders large snapshots", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setContactsSnapshot({
    contacts: Array.from({ length: 95 }, (_, index) => ({
      id: `contact-lazy-${index}`,
      name: `Lazy Contact ${index}`,
      description: `Contact ${index} should not render until its batch is loaded.`,
      avatar: null,
      contact_ids: [`telegram@lazy${index}`]
    })),
    candidates: []
  });
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  const list = page.getByLabel("Contact list");
  await expect(list.locator(".pf-task-row")).toHaveCount(40);
  await expect(list).not.toContainText("Lazy Contact 94");
  await list.getByRole("button", { name: "Load 55 more contacts" }).scrollIntoViewIfNeeded();
  await expect(list.locator(".pf-task-row")).toHaveCount(80);
  await list.getByRole("button", { name: "Load 15 more contacts" }).scrollIntoViewIfNeeded();
  await expect(list.locator(".pf-task-row")).toHaveCount(95);
  await expect(list).toContainText("Lazy Contact 94");
});

test("contacts avatars render without exposing raw data URIs", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setContactsSnapshot({
    contacts: [
      {
        id: "contact-alice",
        name: "Alice",
        description: "Alice sends actionable deployment and support questions.",
        avatar: ALICE_AVATAR,
        contact_ids: ["telegram@alice"]
      }
    ],
    candidates: []
  });
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  const row = page.locator(".pf-task-row").filter({ hasText: "Alice" });
  await expect(row.locator(".pf-contact-avatar img")).toHaveAttribute("src", ALICE_AVATAR);
  await expect(row).toContainText("Avatar saved");
  await expect(row).not.toContainText(ALICE_AVATAR);

  const selectedContact = page.getByRole("complementary", { name: "Selected contact" });
  await expect(selectedContact.locator(".pf-contact-avatar img")).toHaveAttribute("src", ALICE_AVATAR);
  await expect(selectedContact).toContainText("Avatar saved");
  await expect(selectedContact).not.toContainText(ALICE_AVATAR);
});

test("contacts infer modal reruns only from explicit action", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setContactsSnapshot({
    contacts: [],
    candidates: [
      {
        id: "telegram@alice",
        name: "Alice",
        avatar: ALICE_AVATAR,
        score: 42,
        context: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  await page.getByRole("button", { name: "Infer", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Infer contacts" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Rerun" })).toBeVisible();
  await expect(dialog).toContainText("No inferred contacts yet.");
  await page.waitForTimeout(250);
  expect(daemon.requests.filter((request) => request.method === "contacts_infer")).toHaveLength(0);

  await dialog.getByRole("button", { name: "Rerun" }).click();
  const inferRequest = await daemon.waitForRequest("contacts_infer");
  expect(inferRequest.params.limit).toBe(30);
  await expect(dialog).toContainText("1 proposal");
  await expect(dialog.locator(".pf-contact-proposal")).toHaveCount(1);
  await expect(dialog.locator(".pf-contact-proposal").filter({ hasText: "Alice" })).toHaveCount(1);
  await expect(dialog.locator(".pf-contact-proposal img")).toHaveAttribute("src", ALICE_AVATAR);
  await expect(page.getByRole("heading", { name: "Contacts 0" })).toBeVisible();

  await dialog.getByRole("button", { name: "Close inferred contacts" }).click();
  await page.getByRole("button", { name: "Infer", exact: true }).click();
  await expect(dialog).toContainText("1 proposal");
  await page.waitForTimeout(250);
  expect(daemon.requests.filter((request) => request.method === "contacts_infer")).toHaveLength(1);

  daemon.delayResponse("contacts_infer", () => true, 750);
  await dialog.getByRole("button", { name: "Rerun" }).click();
  await expect(dialog.getByRole("button", { name: "Inferring", exact: true })).toBeVisible();
  await expect
    .poll(() => daemon.requests.filter((request) => request.method === "contacts_infer").length)
    .toBe(2);
  await expect(dialog.getByRole("button", { name: "Rerun" })).toBeVisible();
  await expect(dialog.locator(".pf-contact-proposal")).toHaveCount(1);
  await expect(dialog).toContainText("1 proposal");

  await dialog.getByRole("button", { name: "Close inferred contacts" }).click();
  const contactListCount = daemon.requests.filter((request) => request.method === "contacts_list").length;
  await page.reload();
  await openContacts(page);
  await expect
    .poll(() => daemon.requests.filter((request) => request.method === "contacts_list").length)
    .toBeGreaterThan(contactListCount);
  await expect(page.getByRole("heading", { name: "Contacts 0" })).toBeVisible();
  await page.getByRole("button", { name: "Infer", exact: true }).click();
  const reloadedDialog = page.getByRole("dialog", { name: "Infer contacts" });
  await expect(reloadedDialog.locator(".pf-contact-proposal")).toHaveCount(1);
  await page.waitForTimeout(250);
  expect(daemon.requests.filter((request) => request.method === "contacts_infer")).toHaveLength(2);
});

test("contacts infer presents proposals until the user saves", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setContactsSnapshot({
    contacts: [],
    candidates: [
      {
        id: "telegram@alice",
        name: "Alice",
        avatar: ALICE_AVATAR,
        score: 42,
        context: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  await page.getByRole("button", { name: "Infer", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Infer contacts" });
  await dialog.getByRole("button", { name: "Rerun" }).click();
  await daemon.waitForRequest("contacts_infer");
  await expect(dialog).toContainText("1 proposal");
  await expect(dialog.locator(".pf-contact-proposal")).toHaveCount(1);
  await expect(page.getByRole("heading", { name: "Contacts 0" })).toBeVisible();
  expect(daemon.requests.filter((request) => request.method === "contacts_save")).toHaveLength(0);

  await dialog.getByRole("button", { name: "Use" }).click();
  const createDialog = page.getByRole("dialog", { name: "Create contact" });
  await expect(createDialog.getByLabel("Name")).toHaveValue("Alice");
  await createDialog.getByRole("button", { name: /^Create$/ }).click();
  const saveRequest = await daemon.waitForRequest("contacts_save");
  expect(saveRequest.params.contact_ids).toEqual(["telegram@alice"]);
  await expect(page.getByRole("heading", { name: "Contacts 1" })).toBeVisible();
  const selectedContact = page.getByRole("complementary", { name: "Selected contact" });
  await expect(selectedContact).toContainText("Alice");
  await expect(selectedContact).toContainText("telegram@alice");
});

test("contacts infer deduplicates repeated candidate identities", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setContactsSnapshot({
    contacts: [],
    candidates: [
      {
        id: "telegram@alice",
        name: "Alice",
        avatar: ALICE_AVATAR,
        score: 42,
        context: []
      },
      {
        id: "Telegram@@Alice",
        name: "Alice Duplicate",
        avatar: null,
        score: 41,
        context: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  await page.getByRole("button", { name: "Infer", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Infer contacts" });
  await dialog.getByRole("button", { name: "Rerun" }).click();
  await daemon.waitForRequest("contacts_infer");

  await expect(dialog).toContainText("1 proposal");
  await expect(dialog.locator(".pf-contact-proposal")).toHaveCount(1);
  await expect(dialog.locator(".pf-contact-proposal").filter({ hasText: "Alice" })).toHaveCount(1);
  await expect(page.getByRole("heading", { name: "Contacts 0" })).toBeVisible();
});

test("contacts infer preserves saved contacts for legacy proposal responses", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setLegacyContactInferResponse();
  daemon.setContactsSnapshot({
    contacts: [
      {
        id: "contact-alice",
        name: "Alice",
        description: "Alice should remain saved when inference returns legacy proposals.",
        avatar: null,
        contact_ids: ["telegram@alice"]
      }
    ],
    candidates: [
      {
        id: "google@bob@example.com",
        name: "Bob",
        avatar: null,
        score: 21,
        context: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  await page.getByRole("button", { name: "Infer", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Infer contacts" });
  await dialog.getByRole("button", { name: "Rerun" }).click();
  await daemon.waitForRequest("contacts_infer");

  await expect(page.getByRole("heading", { name: "Contacts 1" })).toBeVisible();
  await expect(dialog.locator(".pf-contact-proposal")).toHaveCount(1);
  await expect(dialog).toContainText("Bob");

  await dialog.getByRole("button", { name: "Use" }).click();
  const createDialog = page.getByRole("dialog", { name: "Create contact" });
  await expect(createDialog.getByLabel("Name")).toHaveValue("Bob");
  await createDialog.getByRole("button", { name: /^Create$/ }).click();
  const saveRequest = await daemon.waitForRequest("contacts_save");
  expect(saveRequest.params.contact_ids).toEqual(["google@bob@example.com"]);
  await expect(page.getByRole("heading", { name: "Contacts 2" })).toBeVisible();
});

test("contacts infer skips contacts that are already saved", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  await page.getByRole("button", { name: "Infer", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Infer contacts" });
  await dialog.getByRole("button", { name: "Rerun" }).click();
  await daemon.waitForRequest("contacts_infer");

  await expect(dialog).toContainText("1 proposal");
  await expect(dialog.locator(".pf-contact-proposal")).toHaveCount(1);
  await expect(dialog.locator(".pf-contact-proposal").filter({ hasText: "Alice" })).toHaveCount(0);
  await expect(dialog.locator(".pf-contact-proposal").filter({ hasText: "bob@example.com" })).toHaveCount(1);
  await expect(page.getByRole("heading", { name: "Contacts 1" })).toBeVisible();
});
