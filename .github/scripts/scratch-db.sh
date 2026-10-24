#!/usr/bin/env bash
# The scratch-database guard (P5.4, D-067; P5.2 QA's F2): prints the database a Postgres URL
# connects to when it's one of the allowed names, and refuses anything else.
#
# sqlx reads more than the path: a query can name another database (`?dbname=voidforge`) or hide
# one behind a `/` (`…/voidforge_test?application_name=/voidforge_e2e_1`, P5.3's R2-B4), and the
# path is percent-decoded. So a URL with any `?`, `#` or `%` is refused outright. Without them the
# path's last segment is exactly the database sqlx connects to, and that is what's checked.
#
# Usage: scratch-db.sh <url> <pattern>...   (shell glob patterns, for example 'voidforge_e2e_*')
# Exit codes: 0 allowed (the name on stdout); 2 refused (why on stderr).
set -uo pipefail

url="${1-}"
shift || true
refuse() { echo "error: refusing database URL: $*" >&2; exit 2; }
[ -n "$url" ] && [ "$#" -gt 0 ] || refuse "usage: scratch-db.sh <url> <pattern>..."
case "$url" in
    *[?#%]*) refuse "a query, fragment or escape can name another database than the path; use a plain URL" ;;
    postgres://*/* | postgresql://*/*) ;;
    *) refuse "not a postgres:// URL with a database" ;;
esac
db="${url##*/}"
[ -n "$db" ] || refuse "no database named"
for pattern in "$@"; do
    # shellcheck disable=SC2254
    case "$db" in $pattern) echo "$db"; exit 0 ;; esac
done
refuse "$db isn't one of: $*"
