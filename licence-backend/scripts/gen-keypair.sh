#!/usr/bin/env bash
# Generate the P-256 keypair strivo-licence signs/verifies activation JWTs
# with (ES256). Run this once per deployment (or whenever rotating the key)
# — never commit the output.
#
# Usage: scripts/gen-keypair.sh [output-dir]
#
# Produces:
#   <dir>/jwt-private.pem — feed to `wrangler secret put JWT_PRIVATE_KEY`
#   <dir>/jwt-public.pem  — give to the StriVo client as
#                           STRIVO_LICENCE_PUBLIC_KEY (the PEM contents,
#                           verbatim, including header/footer lines)
#
# Rotating the key invalidates every previously issued token immediately
# (clients re-activate against /activate, which re-validates against Lemon
# Squeezy — no data is lost, but every online client needs a network round
# trip before Pro features work again). Don't rotate casually.

set -euo pipefail

if ! command -v openssl >/dev/null 2>&1; then
  echo "error: openssl is required but not on PATH" >&2
  exit 1
fi

out_dir="${1:-.}"
mkdir -p "$out_dir"

priv="$out_dir/jwt-private.pem"
pub="$out_dir/jwt-public.pem"

if [[ -e "$priv" || -e "$pub" ]]; then
  echo "error: $priv or $pub already exists — refusing to overwrite a keypair." >&2
  echo "Move or delete the existing files first if you really mean to rotate." >&2
  exit 1
fi

# genpkey (PKCS8) is what modern openssl produces and what src/jwt.ts's
# fast path expects; src/jwt.ts also accepts SEC1 (openssl ecparam
# -genkey) for compatibility, converting it internally.
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
  -pkeyopt ec_param_enc:named_curve -out "$priv"
openssl ec -in "$priv" -pubout -out "$pub" 2>/dev/null

chmod 600 "$priv"

cat <<EOF
Generated:
  $priv  (private — wrangler secret put JWT_PRIVATE_KEY < $priv)
  $pub   (public  — give the PEM contents to the client as STRIVO_LICENCE_PUBLIC_KEY)

Next steps:
  cd licence-backend
  npx wrangler secret put JWT_PRIVATE_KEY < $priv

Then delete $priv from disk once it's stored as a Worker secret — it does
not need to live on your machine after that.
EOF
