#!/usr/bin/env bash
# ===========================================================================
#  A signing identity of this Mac's own, so every build of BuzzAnimate is the
#  same app to macOS.
#
#  Signed ad hoc, each build has a new code hash, and macOS's privacy
#  settings (access to Documents and the like) are kept per hash: every
#  install asked again, and the app sat at launch until it was answered. A
#  certificate that stays the same gives every build the same designated
#  requirement, so a permission given once holds.
#
#  The certificate is self-made, for this Mac only — it proves nothing to
#  anyone else, and is not for distributing the app. It lives in a keychain
#  file of its own beside the login keychain, which this script can unlock
#  without asking, so signing never stops for a password.
#
#      source installer/macos/local_signing.sh
#      local_signing_keychain    # echoes the keychain to sign from, or fails
# ===========================================================================

LOCAL_SIGNING_NAME="BuzzAnimate Local Signing"
LOCAL_SIGNING_KEYCHAIN="$HOME/Library/Keychains/buzzanimate-signing.keychain-db"
# Guards a self-made certificate that signs builds on this Mac and nothing
# else; kept here so the build can unlock it unattended.
LOCAL_SIGNING_PASSWORD="buzzanimate-local-signing"

local_signing_keychain() {
    local kc="$LOCAL_SIGNING_KEYCHAIN" pw="$LOCAL_SIGNING_PASSWORD"
    if [ ! -f "$kc" ]; then
        local tmp
        tmp="$(mktemp -d)"
        # Code signing only, ten years, key and certificate made together.
        cat > "$tmp/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $LOCAL_SIGNING_NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
        openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$tmp/cert.cnf" \
            -keyout "$tmp/key.pem" -out "$tmp/cert.pem" >/dev/null 2>&1 || { rm -rf "$tmp"; return 1; }
        openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
            -name "$LOCAL_SIGNING_NAME" -passout "pass:$pw" -out "$tmp/id.p12" >/dev/null 2>&1 || { rm -rf "$tmp"; return 1; }
        security create-keychain -p "$pw" "$kc" >/dev/null || { rm -rf "$tmp"; return 1; }
        # Never locks itself while a build is running.
        security set-keychain-settings "$kc"
        security unlock-keychain -p "$pw" "$kc"
        security import "$tmp/id.p12" -k "$kc" -P "$pw" -T /usr/bin/codesign >/dev/null || { rm -rf "$tmp"; rm -f "$kc"; return 1; }
        # codesign may use the key without a dialog.
        security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$pw" "$kc" >/dev/null 2>&1
        rm -rf "$tmp"
    fi
    security unlock-keychain -p "$pw" "$kc" || return 1
    echo "$kc"
}
