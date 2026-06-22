#!/usr/bin/env bash
#
# Dev-only macOS code signing for the 1Password CLI import path.
#
# WHY: 1Password's macOS app integration only grants the `op` CLI a delegated
# session when the process that *invoked* op is code-signed. `cargo build`
# produces ad-hoc-signed binaries, which 1Password rejects
# ("RequestDelegatedSession: cannot setup session"), so puffer's daemon cannot
# use the op-CLI import path. This script signs the daemon + the desktop host
# with a persistent self-signed certificate so that path can be tested locally.
#
# FOR LOCAL TEST REPRODUCTION ONLY. Production macOS builds are signed with an
# Apple Developer ID certificate (and notarized) in the release pipeline; this
# self-signed dev certificate is never shipped.
#
# Re-run after every `cargo build --release` (cargo reverts the binaries to
# ad-hoc). The first op-CLI sync after signing prompts once to authorize the
# certificate in 1Password (Touch ID); the same cert keeps that authorization
# across rebuilds, since the script reuses it.
#
set -euo pipefail

CERT_NAME="Puffer Dev Signing"
KEYCHAIN="${PUFFER_SIGN_KEYCHAIN:-$HOME/Library/Keychains/puffer-dev-signing.keychain-db}"
KEYCHAIN_PW="puffer-dev"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARIES=(
  "$REPO_ROOT/target/release/puffer"
  "$REPO_ROOT/apps/puffer-desktop/src-tauri/target/release/corbina"
)

create_cert() {
  local work
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' RETURN
  echo "==> Creating self-signed code-signing certificate '$CERT_NAME'"
  openssl req -x509 -newkey rsa:2048 -keyout "$work/key.pem" -out "$work/cert.pem" \
    -days 3650 -nodes -subj "/CN=$CERT_NAME" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" >/dev/null 2>&1
  # Legacy PBE so macOS `security` can import the PKCS#12 (OpenSSL 3 defaults
  # to a MAC algorithm `security` cannot verify).
  openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$work/cert.p12" -passout "pass:puffer" -name "$CERT_NAME" \
    -legacy -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 >/dev/null 2>&1
  # Dedicated keychain with a known password so signing is non-interactive.
  security delete-keychain "$KEYCHAIN" 2>/dev/null || true
  security create-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"
  security set-keychain-settings "$KEYCHAIN" # disable the auto-lock timeout
  security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"
  security import "$work/cert.p12" -k "$KEYCHAIN" -P "puffer" -T /usr/bin/codesign -A >/dev/null
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PW" "$KEYCHAIN" >/dev/null
}

if ! security find-identity -p codesigning "$KEYCHAIN" 2>/dev/null | grep -q "$CERT_NAME"; then
  create_cert
else
  security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"
fi

echo "==> Signing with '$CERT_NAME'"
for bin in "${BINARIES[@]}"; do
  if [[ ! -f "$bin" ]]; then
    echo "  !! not built yet, skipping: ${bin#"$REPO_ROOT"/}" >&2
    continue
  fi
  codesign --force --keychain "$KEYCHAIN" --sign "$CERT_NAME" "$bin"
  echo "  signed: ${bin#"$REPO_ROOT"/}"
done

cat <<EOF

Done. Launch bobo from your authorized Terminal and run "Sync from 1Password":
the first sync prompts once to authorize "$CERT_NAME" in 1Password (Touch ID).

NOTE: dev/test reproduction only — production macOS builds are signed with an
Apple Developer ID certificate (and notarized) in the release pipeline.
EOF
