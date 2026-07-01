#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

CACHE=".cache/open-props.min.css"
mkdir -p .cache

if [ ! -f "$CACHE" ]; then
  echo "Downloading Open Props..."
  curl -sL https://unpkg.com/open-props/open-props.min.css -o "$CACHE"
fi

USED=$(grep -roh --include='*.rs' --include='*.css' 'var(--[a-zA-Z0-9-]*)' src/ \
       | sed 's/var(--//;s/)//' | sort -u)

{
  echo "/* Open Props JIT — $(date) */"
  echo -n ":where(html){"
  first=true
  while IFS= read -r prop; do
    decl=$(grep -oE -- "--${prop}:[^;]*;" "$CACHE" 2>/dev/null || true)
    if [ -n "$decl" ]; then
      $first || echo -n " "
      echo -n "$decl"
      first=false
    fi
  done <<< "$USED"
  echo "}"
} > src/open-props-jit.css
