# Sourced by the nightly scripts (slice 53, D-066): one bounded wait for every readiness probe.
# A service that compose or GitHub reports healthy may still be seconds from answering (night 7:
# Grafana's Prometheus proxy, issue #65), so a probe retries until its deadline, and the infra
# message says how long it waited. Only readiness waits; a check is never retried.
#
# ready_within <seconds> <interval> <probe...>
#   Runs the probe every <interval> seconds until it succeeds or <seconds> have passed since the
#   first attempt. Returns 0 once it succeeds; 1 at the deadline, or at once when the probe exits
#   125 (nothing left to wait for, such as a process that died). Sets `waited` to the whole
#   seconds spent. A probe bounds its own attempt (curl -m, timeout).
# NIGHTLY_READY_WAIT is the services' deadline in seconds (default 120).
ready_wait="${NIGHTLY_READY_WAIT:-120}"
case "$ready_wait" in ''|*[!0-9]*) echo "NIGHTLY_READY_WAIT must be whole seconds" >&2; exit 2 ;; esac
ready_within() {
    local bound=$1 interval=$2 start=$SECONDS code
    shift 2
    while :; do
        "$@"
        code=$?
        waited=$(( SECONDS - start ))
        [ "$code" = 0 ] && return 0
        [ "$code" = 125 ] && return 1
        [ "$waited" -ge "$bound" ] && return 1
        sleep "$interval"
    done
}
