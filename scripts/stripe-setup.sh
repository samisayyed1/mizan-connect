#!/usr/bin/env bash
# Mizan Connect — one-shot Stripe setup.
#
# Prereqs (one-time):
#   1. The three Products already created in test mode:
#        "Mizan Basic"  with Monthly + Yearly recurring Prices
#        "Mizan Pro"    with Monthly + Yearly recurring Prices
#        "Mizan Enterprise" with Monthly + Yearly recurring Prices
#   2. Stripe CLI installed + logged in:
#        brew install stripe/stripe-cli/stripe
#        stripe login
#
# What this script does:
#   1. Finds the six Prices by Product name + recurring.interval.
#   2. Tags each Price with metadata.plan = basic|pro|enterprise.
#   3. (Optional) Creates a Stripe webhook endpoint at the URL you pass.
#   4. Prints a ready-to-paste `fly secrets set …` command with every value.
#
# Usage:
#   ./scripts/stripe-setup.sh                                # tag prices + print env
#   ./scripts/stripe-setup.sh https://<your-app>.fly.dev     # also create webhook
#
# Re-running is idempotent (metadata updates are PATCHes; webhook creation will
# 409 if one already exists, which is fine).

set -euo pipefail

WEBHOOK_BASE="${1:-}"

# ── 0. sanity ───────────────────────────────────────────────────────────────
command -v stripe >/dev/null || { echo "stripe CLI not found. brew install stripe/stripe-cli/stripe" >&2; exit 1; }
command -v jq >/dev/null     || { echo "jq not found. brew install jq" >&2; exit 1; }
stripe products list --limit 1 >/dev/null 2>&1 || { echo "stripe login required: run \`stripe login\`" >&2; exit 1; }

echo "→ Fetching products + prices from Stripe (test mode)…"

PRODUCTS_JSON=$(stripe products list --limit 100 -d "active=true")

# ── 1. resolve product ids by name ──────────────────────────────────────────
find_product_id() {
  local needle="$1"
  echo "$PRODUCTS_JSON" \
    | jq -r --arg n "$needle" '.data[] | select(.name == $n) | .id' \
    | head -1
}

P_BASIC=$(find_product_id "Mizan Basic")
P_PRO=$(find_product_id "Mizan Pro")
P_ENT=$(find_product_id "Mizan Enterprise")

for pair in "Mizan Basic:$P_BASIC" "Mizan Pro:$P_PRO" "Mizan Enterprise:$P_ENT"; do
  name=${pair%%:*}; id=${pair##*:}
  if [[ -z "$id" ]]; then
    echo "  ✗ product not found: $name. Create it in the Stripe Dashboard (test mode)." >&2
    exit 1
  fi
  echo "  ✓ $name → $id"
done

# ── 2. find Monthly + Yearly Price under each product ───────────────────────
find_price_id() {
  local product_id="$1" interval="$2"
  stripe prices list --limit 100 -d "active=true" -d "product=$product_id" \
    | jq -r --arg iv "$interval" '.data[] | select(.recurring.interval == $iv and .recurring.interval_count == 1) | .id' \
    | head -1
}

echo "→ Resolving Monthly + Yearly Price IDs…"

PRICE_BASIC_M=$(find_price_id "$P_BASIC" "month")
PRICE_BASIC_Y=$(find_price_id "$P_BASIC" "year")
PRICE_PRO_M=$(find_price_id   "$P_PRO"   "month")
PRICE_PRO_Y=$(find_price_id   "$P_PRO"   "year")
PRICE_ENT_M=$(find_price_id   "$P_ENT"   "month")
PRICE_ENT_Y=$(find_price_id   "$P_ENT"   "year")

for pair in \
  "BASIC_MONTHLY:$PRICE_BASIC_M" "BASIC_YEARLY:$PRICE_BASIC_Y" \
  "PRO_MONTHLY:$PRICE_PRO_M"     "PRO_YEARLY:$PRICE_PRO_Y" \
  "ENT_MONTHLY:$PRICE_ENT_M"     "ENT_YEARLY:$PRICE_ENT_Y" ; do
  slot=${pair%%:*}; id=${pair##*:}
  if [[ -z "$id" ]]; then
    echo "  ✗ no Price found for $slot. Each Product needs both a Monthly and a Yearly recurring Price." >&2
    exit 1
  fi
  echo "  ✓ $slot → $id"
done

# ── 3. tag each Price with metadata.plan ────────────────────────────────────
echo "→ Tagging each Price with metadata.plan …"
tag() {
  local id="$1" plan="$2"
  stripe prices update "$id" -d "metadata[plan]=$plan" >/dev/null
  echo "  ✓ $id  metadata.plan=$plan"
}
tag "$PRICE_BASIC_M" basic
tag "$PRICE_BASIC_Y" basic
tag "$PRICE_PRO_M"   pro
tag "$PRICE_PRO_Y"   pro
tag "$PRICE_ENT_M"   enterprise
tag "$PRICE_ENT_Y"   enterprise

# ── 4. webhook endpoint (optional) ──────────────────────────────────────────
WEBHOOK_SECRET=""
if [[ -n "$WEBHOOK_BASE" ]]; then
  WEBHOOK_URL="${WEBHOOK_BASE%/}/v1/stripe/webhook"
  echo "→ Creating webhook endpoint at $WEBHOOK_URL …"

  # If an endpoint at this URL already exists, reuse its secret.
  EXISTING=$(stripe webhook_endpoints list --limit 100 \
    | jq -r --arg u "$WEBHOOK_URL" '.data[] | select(.url == $u) | .id' | head -1)

  if [[ -n "$EXISTING" ]]; then
    echo "  ↻ endpoint exists ($EXISTING); reusing. (Secret is only revealable at creation —"
    echo "    if you don't have it saved, delete the endpoint in the Dashboard and re-run.)"
  else
    CREATED=$(stripe webhook_endpoints create \
      -d "url=$WEBHOOK_URL" \
      -d "enabled_events[]=customer.subscription.created" \
      -d "enabled_events[]=customer.subscription.updated" \
      -d "enabled_events[]=customer.subscription.deleted" \
      -d "enabled_events[]=invoice.paid" \
      -d "enabled_events[]=checkout.session.completed")
    WEBHOOK_SECRET=$(echo "$CREATED" | jq -r '.secret')
    echo "  ✓ created. signing secret saved below."
  fi
fi

# ── 5. print the secrets bundle ─────────────────────────────────────────────
echo
echo "─────────────────────────────────────────────────────────────────────"
echo "Paste this into a scratch file (or feed it to \`fly secrets set\`):"
echo "─────────────────────────────────────────────────────────────────────"
cat <<EOF
STRIPE_PRICE_BASIC_MONTHLY=$PRICE_BASIC_M
STRIPE_PRICE_BASIC_YEARLY=$PRICE_BASIC_Y
STRIPE_PRICE_PRO_MONTHLY=$PRICE_PRO_M
STRIPE_PRICE_PRO_YEARLY=$PRICE_PRO_Y
STRIPE_PRICE_ENTERPRISE_MONTHLY=$PRICE_ENT_M
STRIPE_PRICE_ENTERPRISE_YEARLY=$PRICE_ENT_Y
EOF

if [[ -n "$WEBHOOK_SECRET" ]]; then
  echo "STRIPE_WEBHOOK_SECRET=$WEBHOOK_SECRET"
fi

cat <<'EOF'

You still need (NOT auto-discovered for safety reasons):
  STRIPE_SECRET_KEY=sk_test_…       (Dashboard → Developers → API keys)
  OPENAI_API_KEY=sk-…               (only if you want managed AI live)
  MIZAN_BILLING_RETURN_URL=mizan://billing/return

Once you have all of them, the full `fly secrets set` is:
  fly secrets set STRIPE_SECRET_KEY=… STRIPE_WEBHOOK_SECRET=… \
    STRIPE_PRICE_BASIC_MONTHLY=… STRIPE_PRICE_BASIC_YEARLY=… \
    STRIPE_PRICE_PRO_MONTHLY=… STRIPE_PRICE_PRO_YEARLY=… \
    STRIPE_PRICE_ENTERPRISE_MONTHLY=… STRIPE_PRICE_ENTERPRISE_YEARLY=… \
    OPENAI_API_KEY=… MIZAN_BILLING_RETURN_URL=mizan://billing/return \
    --app <your-fly-app-name>
EOF
