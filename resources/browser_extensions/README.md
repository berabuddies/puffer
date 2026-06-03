# Browser Extension Resources

Puffer keeps browser extension package metadata in the config catalog and loads
bundled extension directories from this resource tree.

- `nopecha/chromium_automation`: NopeCHA Chromium automation extension 0.6.0.
  Source: https://github.com/NopeCHALLC/nopecha-extension/releases/tag/0.6.0.
  License: MIT.
- `2captcha/chromium`: 2Captcha Chrome solver extension 3.7.2.
  Source: https://github.com/rucaptcha/2captcha-solver/releases/tag/v3.7.2.
  License: MIT.

Runtime settings store only solver selection, base URLs, and encrypted secret
IDs for API keys. Raw API keys belong in the Puffer secret vault.
