const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])"
].join(",");

function isFocusable(element: Element): element is HTMLElement {
  if (!(element instanceof HTMLElement)) return false;
  if (element.matches("[disabled], [aria-disabled='true']")) return false;
  const style = window.getComputedStyle(element);
  if (style.visibility === "hidden" || style.display === "none") return false;
  return element.getClientRects().length > 0;
}

function focusableElements(node: HTMLElement): HTMLElement[] {
  return Array.from(node.querySelectorAll(FOCUSABLE_SELECTOR)).filter(isFocusable);
}

function focusInitial(node: HTMLElement) {
  const preferred = node.querySelector<HTMLElement>("[data-autofocus], [autofocus]");
  const first = preferred && isFocusable(preferred)
    ? preferred
    : focusableElements(node)[0] ?? node;
  first.focus({ preventScroll: true });
}

function restoreFocus(element: HTMLElement | null) {
  if (!element?.isConnected) return;
  element.focus({ preventScroll: true });
  window.requestAnimationFrame(() => {
    if (document.querySelector("[role='dialog'][aria-modal='true']")) return;
    if (element.isConnected && document.activeElement !== element) {
      element.focus({ preventScroll: true });
    }
  });
}

/** Keeps keyboard focus inside a modal dialog while it is mounted. */
export function focusTrap(node: HTMLElement) {
  const previousFocus = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : null;
  const initialTimer = window.setTimeout(() => focusInitial(node), 0);

  function onKeydown(event: KeyboardEvent) {
    if (event.key !== "Tab") return;
    const focusables = focusableElements(node);
    if (focusables.length === 0) {
      event.preventDefault();
      node.focus({ preventScroll: true });
      return;
    }

    const current = document.activeElement;
    const currentIndex = current instanceof HTMLElement ? focusables.indexOf(current) : -1;
    const nextIndex = event.shiftKey
      ? currentIndex <= 0
        ? focusables.length - 1
        : currentIndex - 1
      : currentIndex === -1 || currentIndex === focusables.length - 1
        ? 0
        : currentIndex + 1;

    event.preventDefault();
    focusables[nextIndex].focus({ preventScroll: true });
  }

  function onFocusIn(event: FocusEvent) {
    if (event.target instanceof Node && node.contains(event.target)) return;
    focusInitial(node);
  }

  node.addEventListener("keydown", onKeydown);
  document.addEventListener("focusin", onFocusIn);

  return {
    destroy() {
      window.clearTimeout(initialTimer);
      node.removeEventListener("keydown", onKeydown);
      document.removeEventListener("focusin", onFocusIn);
      restoreFocus(previousFocus);
    }
  };
}
