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
  expect(request.params.contact_ids).toEqual(["telegram@alice", "google@alice@example.com"]);
  await expect(selectedContact).toContainText("Launch Alice");
});

test("contacts save selects the backend-normalized saved contact", async ({ page }) => {
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
  await dialog.getByLabel("Contact IDs").fill("Telegram@@Casey");
  await dialog.getByRole("button", { name: /^Create$/ }).click();

  const request = await daemon.waitForRequest("contacts_save");
  expect(request.params.contact_ids).toEqual(["Telegram@@Casey"]);
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
  expect(request.params.contact_ids).toEqual(["Telegram@@Alice"]);
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
    contacts: Array.from({ length: 65 }, (_, index) => ({
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
  await expect(list).not.toContainText("Lazy Contact 64");
  await list.getByRole("button", { name: "Load 25 more contacts" }).scrollIntoViewIfNeeded();
  await expect(list.locator(".pf-task-row")).toHaveCount(65);
  await expect(list).toContainText("Lazy Contact 64");
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

  await page.getByRole("button", { name: "Infer" }).click();
  const dialog = page.getByRole("dialog", { name: "Infer contacts" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Rerun" })).toBeVisible();
  await expect(dialog).toContainText("No inferred contacts yet.");
  await page.waitForTimeout(250);
  expect(daemon.requests.filter((request) => request.method === "contacts_infer")).toHaveLength(0);

  await dialog.getByRole("button", { name: "Rerun" }).click();
  const inferRequest = await daemon.waitForRequest("contacts_infer");
  expect(inferRequest.params.limit).toBe(30);
  await expect(dialog).toContainText("Alice");
  await expect(dialog.locator(".pf-contact-proposal img")).toHaveAttribute("src", ALICE_AVATAR);

  await dialog.getByRole("button", { name: "Close inferred contacts" }).click();
  await page.getByRole("button", { name: "Infer" }).click();
  await expect(dialog).toContainText("Alice");
  await page.waitForTimeout(250);
  expect(daemon.requests.filter((request) => request.method === "contacts_infer")).toHaveLength(1);

  daemon.delayResponse("contacts_infer", () => true, 750);
  await dialog.getByRole("button", { name: "Rerun" }).click();
  await expect(dialog.getByRole("button", { name: "Inferring", exact: true })).toBeVisible();
  await expect(dialog).toContainText("Alice");
  await expect
    .poll(() => daemon.requests.filter((request) => request.method === "contacts_infer").length)
    .toBe(2);
  await expect(dialog.getByRole("button", { name: "Rerun" })).toBeVisible();

  await dialog.getByRole("button", { name: "Close inferred contacts" }).click();
  const contactListCount = daemon.requests.filter((request) => request.method === "contacts_list").length;
  await page.reload();
  await openContacts(page);
  await expect
    .poll(() => daemon.requests.filter((request) => request.method === "contacts_list").length)
    .toBeGreaterThan(contactListCount);
  await page.getByRole("button", { name: "Infer" }).click();
  const reloadedDialog = page.getByRole("dialog", { name: "Infer contacts" });
  await expect(reloadedDialog).toContainText("Alice");
  await expect(reloadedDialog.locator(".pf-contact-proposal img")).toHaveAttribute("src", ALICE_AVATAR);
  await page.waitForTimeout(250);
  expect(daemon.requests.filter((request) => request.method === "contacts_infer")).toHaveLength(2);
});

test("contacts infer removes a proposal after it is saved", async ({ page }) => {
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

  await page.getByRole("button", { name: "Infer" }).click();
  const dialog = page.getByRole("dialog", { name: "Infer contacts" });
  await dialog.getByRole("button", { name: "Rerun" }).click();
  await daemon.waitForRequest("contacts_infer");
  await expect(dialog).toContainText("Alice");

  await dialog.getByRole("button", { name: "Use" }).click();
  const createDialog = page.getByRole("dialog", { name: "Create contact" });
  await expect(createDialog.getByLabel("Name")).toHaveValue("Alice");
  await expect(createDialog.getByLabel("Contact IDs")).toHaveValue("telegram@alice");
  await createDialog.getByRole("button", { name: /^Create$/ }).click();

  const request = await daemon.waitForRequest("contacts_save");
  expect(request.params.contact_ids).toEqual(["telegram@alice"]);
  await page.getByRole("button", { name: "Infer" }).click();
  const refreshedDialog = page.getByRole("dialog", { name: "Infer contacts" });
  await expect(refreshedDialog.locator(".pf-contact-proposal")).toHaveCount(0);
  await expect(refreshedDialog).toContainText("No inferred contacts yet.");
});

test("contacts infer skips contacts that are already saved", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openContacts(page);
  await daemon.waitForRequest("contacts_list");

  await page.getByRole("button", { name: "Infer" }).click();
  const dialog = page.getByRole("dialog", { name: "Infer contacts" });
  await dialog.getByRole("button", { name: "Rerun" }).click();
  await daemon.waitForRequest("contacts_infer");

  const proposals = dialog.locator(".pf-contact-proposal");
  await expect(proposals).toHaveCount(1);
  await expect(proposals).toContainText("bob@example.com");
  await expect(proposals).not.toContainText("Alice");
});
