#!/usr/bin/env bash
# Lists candidate Ladybird PRs that match Plan 3's selection criteria.
#
# Output: one PR number per line. Pipe into `tee` and review by hand;
# this script does not commit anything to the corpus.
#
# Criteria enforced by this script (subset of spec §3.1):
#   - Merged into LadybirdBrowser/ladybird master
#   - At least 6 months old (>= 180 days since merge)
#   - Modifies at most 5 source files (test files don't count toward this)
#   - At least one new/modified file under Tests/
#
# Manual filters that this script CANNOT enforce (review by hand):
#   - Clear root cause described in the PR/issue
#   - Builds in standard Ladybird Docker image (no special hardware deps)
#   - Domain distribution per spec §3.2

set -euo pipefail

REPO="LadybirdBrowser/ladybird"
SINCE_DAYS="${SINCE_DAYS:-180}"
LIMIT="${LIMIT:-200}"

cutoff=$(date -u -v -"${SINCE_DAYS}d" +"%Y-%m-%dT%H:%M:%SZ" 2>/dev/null \
       || date -u -d "${SINCE_DAYS} days ago" +"%Y-%m-%dT%H:%M:%SZ")

gh pr list \
  --repo "$REPO" \
  --state merged \
  --limit "$LIMIT" \
  --search "merged:<${cutoff}" \
  --json number,title,mergedAt,files,url \
  | jq -r '.[]
      | select(
          (.files | map(select(.path | test("^Tests/"))) | length) >= 1
          and
          (.files | map(select(.path | test("^Tests/") | not)) | length) <= 5
        )
      | "\(.number)\t\(.title)\t\(.url)"'
