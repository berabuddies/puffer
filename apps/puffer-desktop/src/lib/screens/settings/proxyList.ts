import type { NetworkProxySettings } from "../../types";

export function normalizeProxySettingsForSave(proxy: NetworkProxySettings): NetworkProxySettings {
  const selectedExists = Boolean(
    proxy.selected && proxy.proxies.some((item) => item.id === proxy.selected)
  );
  let selected = selectedExists ? proxy.selected : null;
  let enabled = proxy.enabled;

  if (proxy.proxies.length === 0) {
    enabled = false;
    selected = null;
  } else if (enabled && !selected) {
    selected = proxy.proxies[0].id;
  }

  const testedProxyStillExists = Boolean(
    proxy.lastTest?.proxyId && proxy.proxies.some((item) => item.id === proxy.lastTest?.proxyId)
  );
  const lastTest = proxy.lastTest?.proxyId && !testedProxyStillExists ? null : proxy.lastTest;

  return {
    ...proxy,
    enabled,
    selected,
    lastTest
  };
}

export function proxySwitchChecked(proxy: NetworkProxySettings): boolean {
  return normalizeProxySettingsForSave(proxy).enabled;
}

export function proxySwitchDisabled(proxy: NetworkProxySettings): boolean {
  return proxy.proxies.length === 0;
}

export function setProxyEnabled(
  proxy: NetworkProxySettings,
  enabled: boolean
): NetworkProxySettings {
  return normalizeProxySettingsForSave({
    ...proxy,
    enabled
  });
}

export function removeProxyEndpoint(
  proxy: NetworkProxySettings,
  proxyId: string
): NetworkProxySettings {
  const proxies = proxy.proxies.filter((item) => item.id !== proxyId);
  return normalizeProxySettingsForSave({
    ...proxy,
    proxies,
    lastTest: proxy.lastTest?.proxyId === proxyId ? null : proxy.lastTest
  });
}
