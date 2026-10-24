#!/usr/bin/env bash
# Regression cases for scratch-db.sh (P5.4, D-067): every way a URL can name another database than
# its path is refused, and the plain scratch names are allowed. Run by CI's lint job and just lint.
# Exit codes: 0 every case passed; 1 one failed.
set -uo pipefail

guard="$(cd "$(dirname "$0")" && pwd)/scratch-db.sh"
base="postgres://voidforge:pw@localhost:5432"
bad=0
allowed() {
    local got
    if got=$("$guard" "$1" "${@:3}" 2> /dev/null) && [ "$got" = "$2" ]; then
        echo "pass: allows $1"
    else
        echo "FAIL: should allow $1 as $2 (got '${got:-}', exit $?)"; bad=1
    fi
}
refused() {
    if "$guard" "$1" "${@:2}" > /dev/null 2>&1; then
        echo "FAIL: should refuse $1"; bad=1
    else
        [ "$("$guard" "$1" "${@:2}" 2>&1 > /dev/null | grep -c '^error: refusing')" = 1 ] \
            && echo "pass: refuses $1" || { echo "FAIL: $1 refused without its message"; bad=1; }
    fi
}

allowed "$base/voidforge_e2e_20261024_1" voidforge_e2e_20261024_1 voidforge_dev 'voidforge_e2e_*'
allowed "$base/voidforge_dev" voidforge_dev voidforge_dev 'voidforge_e2e_*'
allowed "postgresql://voidforge:pw@localhost:5441/voidforge_drill" voidforge_drill voidforge_drill
# P5.2 QA's F2: a query names another database.
refused "$base/voidforge_e2e_1?dbname=voidforge" voidforge_dev 'voidforge_e2e_*'
# P5.3's R2-B4: a query hides the real database behind a `/`.
refused "$base/voidforge?application_name=/voidforge_e2e_1" voidforge_dev 'voidforge_e2e_*'
# The path is percent-decoded: %76 is v.
refused "$base/%76oidforge" voidforge_dev 'voidforge_e2e_*' '*voidforge'
refused "$base/voidforge_e2e_%31" voidforge_dev 'voidforge_e2e_*'
# A fragment.
refused "$base/voidforge_e2e_1#x" voidforge_dev 'voidforge_e2e_*'
# Not an allowed name, no name, not a postgres URL, no pattern.
refused "$base/voidforge" voidforge_dev 'voidforge_e2e_*'
refused "$base/voidforge_test" voidforge_dev 'voidforge_e2e_*'
refused "$base/" voidforge_dev 'voidforge_e2e_*'
refused "postgres://voidforge:pw@localhost:5432" voidforge_dev 'voidforge_e2e_*'
refused "mysql://h/voidforge_dev" voidforge_dev
refused "$base/voidforge_dev"
[ "$bad" = 0 ] && echo "ok: every scratch-db case passed"
exit "$bad"
