#!/usr/bin/env bash

set -euo pipefail

soak_seconds="${FERESE_SOAK_SECONDS:-86400}"
iteration_delay="${FERESE_SOAK_DELAY:-0.15}"
client_program="${FERESE_SOAK_CLIENT:-foot}"
feresectl_program="${FERESECTL:-target/debug/feresectl}"
compositor_pid="${FERESE_PID:-}"
log_path="${FERESE_SOAK_LOG:-/tmp/ferese-soak-$$.log}"

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "WAYLAND_DISPLAY must name the Ferese socket" >&2
    exit 2
fi

if [[ ! "$soak_seconds" =~ ^[1-9][0-9]*$ ]]; then
    echo "FERESE_SOAK_SECONDS must be a positive integer" >&2
    exit 2
fi

if [[ ! -x "$feresectl_program" ]]; then
    cargo build -p feresectl
fi

if ! command -v "$client_program" >/dev/null 2>&1; then
    echo "soak client not found: $client_program" >&2
    exit 2
fi

if [[ -z "$compositor_pid" ]]; then
    compositor_pid="$(pgrep -n -x ferese || true)"
fi

if [[ -z "$compositor_pid" || ! -r "/proc/$compositor_pid/status" ]]; then
    echo "set FERESE_PID to the running compositor process" >&2
    exit 2
fi

client_pids=()

cleanup() {
    local pid

    for pid in "${client_pids[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill -TERM "$pid" 2>/dev/null || true
        fi
    done
}

trap cleanup EXIT INT TERM

rss_kib() {
    awk '/^VmRSS:/ { print $2; exit }' "/proc/$compositor_pid/status"
}

control() {
    "$feresectl_program" "$@" >/dev/null
}

launch_client() {
    "$client_program" >/dev/null 2>&1 &
    client_pids+=("$!")
}

reap_clients() {
    local pid

    for pid in "${client_pids[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill -TERM "$pid" 2>/dev/null || true
        fi
        wait "$pid" 2>/dev/null || true
    done
    client_pids=()
}

wait_for_focus() {
    local attempt
    local focused

    for ((attempt = 0; attempt < 40; attempt += 1)); do
        focused="$("$feresectl_program" get-focused-window 2>/dev/null || true)"
        if [[ "$focused" != "null" && -n "$focused" ]]; then
            return 0
        fi
        sleep 0.05
    done

    echo "a launched client did not receive focus" >&2
    return 1
}

start_time="$SECONDS"
deadline=$((start_time + soak_seconds))
iteration=0
initial_rss="$(rss_kib)"
maximum_rss="$initial_rss"

{
    echo "ferese native soak"
    echo "pid=$compositor_pid display=$WAYLAND_DISPLAY client=$client_program"
    echo "duration_seconds=$soak_seconds initial_rss_kib=$initial_rss"
} | tee "$log_path"

while ((SECONDS < deadline)); do
    if ! kill -0 "$compositor_pid" 2>/dev/null; then
        echo "FAIL compositor exited at iteration $iteration" | tee -a "$log_path"
        exit 1
    fi

    launch_client
    launch_client
    wait_for_focus

    control focus left
    control focus right
    control resize left
    control resize right
    control cycle-column-width
    control cycle-column-width
    control toggle-fullscreen
    control toggle-fullscreen
    control toggle-floating
    control toggle-floating
    control workspace 2
    control workspace 1
    control close
    sleep "$iteration_delay"
    control close
    sleep "$iteration_delay"

    if ((iteration % 10 == 0)); then
        launch_client
        wait_for_focus
        kill -TERM "${client_pids[-1]}" 2>/dev/null || true
    fi
    reap_clients

    current_rss="$(rss_kib)"
    if ((current_rss > maximum_rss)); then
        maximum_rss="$current_rss"
    fi

    if ((iteration % 100 == 0)); then
        printf 'iteration=%d elapsed=%d rss_kib=%d max_rss_kib=%d\n' \
            "$iteration" "$((SECONDS - start_time))" "$current_rss" "$maximum_rss" \
            | tee -a "$log_path"
    fi

    iteration=$((iteration + 1))
    sleep "$iteration_delay"
done

control get-outputs
control get-workspaces
final_rss="$(rss_kib)"

printf 'PASS iterations=%d final_rss_kib=%d max_rss_kib=%d growth_kib=%d\n' \
    "$iteration" "$final_rss" "$maximum_rss" "$((final_rss - initial_rss))" \
    | tee -a "$log_path"
