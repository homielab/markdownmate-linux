#!/usr/bin/env bash
#
# Upload the signed source packages in dist/ppa/ to the PPA, one series after another.
# Used by .github/workflows/ppa.yml:
#
#   PPA=ppa:homielab/markdownmate DEB_VERSION=1.0.0-0ppa1 packaging/upload-ppa.sh noble resolute
#
# Launchpad's FTP server sometimes answers "550 internal server error" for one upload. A failed
# series is tried again after 2, 5 and 10 minutes (PPA_RETRY_WAITS, in seconds); before each try
# dput's .upload log for that series is removed, or dput would refuse to upload it again. Series
# that went through are not uploaded twice. The script fails, naming the series, only if a
# series still fails after its last try; the other series are uploaded anyway.
#
set -uo pipefail

: "${PPA:?PPA is not set (e.g. ppa:homielab/markdownmate)}"
: "${DEB_VERSION:?DEB_VERSION is not set (e.g. 1.0.0-0ppa1)}"
OUT=${OUT:-dist/ppa}
read -r -a waits <<< "${PPA_RETRY_WAITS:-120 300 600}"

failed=()
for s in "$@"; do
  changes="$OUT/markdownmate_$DEB_VERSION~${s}1_source.changes"
  tries=$((${#waits[@]} + 1))
  for ((try = 1; try <= tries; try++)); do
    if ((try > 1)); then
      wait=${waits[try - 2]}
      echo "::warning::Upload for $s failed (try $((try - 1)) of $tries); retrying in ${wait}s."
      sleep "$wait"
      rm -f "${changes%.changes}".*.upload
    fi
    if dput "$PPA" "$changes"; then
      echo "::notice::Uploaded $DEB_VERSION~${s}1 for $s. Launchpad emails contact@homielab.com when it's accepted (minutes, or hours when busy), then builds it."
      continue 2
    fi
  done
  echo "::error::Upload for $s failed $tries times; re-run later: gh workflow run ppa.yml -f tag=${TAG:-<tag>} (series already in the PPA are skipped)."
  failed+=("$s")
done

if ((${#failed[@]})); then
  echo "Not uploaded: ${failed[*]}" >&2
  exit 1
fi
