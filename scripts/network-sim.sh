#!/bin/bash
# Network simulation script for Hypha testing using dnctl (dummynet)
#
# This script safely configures packet filtering rules to simulate network
# conditions (latency, packet loss, bandwidth limits) on localhost connections
# between Hypha components.
#
# Usage:
#   sudo ./network-sim.sh start [delay_ms] [packet_loss_%] [bandwidth_kbit]
#   sudo ./network-sim.sh status
#   sudo ./network-sim.sh stop
#
# Examples:
#   sudo ./network-sim.sh start 100 5 1000    # 100ms delay, 5% loss, 1Mbps
#   sudo ./network-sim.sh start 50 0 10000    # 50ms delay, no loss, 10Mbps
#   sudo ./network-sim.sh start 200           # 200ms delay only
#   sudo ./network-sim.sh stop                # Remove all rules

set -euo pipefail

# PF anchor name for isolated rule management
# IMPORTANT: must match default dummynet-anchor "com.apple/*" on macOS
ANCHOR="com.apple/hypha-test"
PIPE_NUM=1

show_usage() {
    cat <<EOF
Usage: $0 {start|status|stop} [options]

Commands:
  start [delay] [loss] [bandwidth]
        Start network simulation with optional parameters:
        - delay: Latency in milliseconds (default: 100)
        - loss: Packet loss percentage 0-100 (default: 0)
        - bandwidth: Bandwidth in kbit/s (default: unlimited)

        Examples:
          sudo $0 start 100 5 1000    # 100ms delay, 5% loss, 1Mbps
          sudo $0 start 50            # 50ms delay only

  status
        Show current simulation configuration

  stop
        Remove all simulation rules and restore normal network

Traffic affected:
  All IPv4 and IPv6 traffic on localhost (lo0).
EOF
}

check_root() {
    if [[ $EUID -ne 0 ]]; then
        echo "Error: This script must be run with sudo"
        exit 1
    fi
}

start_simulation() {
    local delay_ms=${1:-100}
    local loss_pct=${2:-0}
    local bw_kbit=${3:-0}

    echo "Starting network simulation..."
    echo "  Delay: ${delay_ms}ms"
    echo "  Packet loss: ${loss_pct}%"
    if [[ $bw_kbit -gt 0 ]]; then
        echo "  Bandwidth: ${bw_kbit}kbit/s"
    else
        echo "  Bandwidth: unlimited"
    fi

    # Build dnctl pipe configuration
    local pipe_config="delay ${delay_ms}ms"

    # Add packet loss if requested (plr expects 0.0 - 1.0)
    if [[ "$loss_pct" -gt 0 ]]; then
        # e.g. 5 -> 0.0500
        local loss_ratio
        loss_ratio=$(bc <<< "scale=4; $loss_pct / 100")
        pipe_config="$pipe_config plr ${loss_ratio}"
    fi

    # Add bandwidth if requested
    if [[ $bw_kbit -gt 0 ]]; then
        pipe_config="$pipe_config bw ${bw_kbit}Kbit/s"
    fi

    # NOTE: Split stats by flow (src/dst/proto/ports)
    pipe_config="$pipe_config mask all"

    echo "Configuring dummynet pipe $PIPE_NUM..."
    dnctl pipe "$PIPE_NUM" config $pipe_config

    echo "Configuring packet filter rules (anchor: $ANCHOR) for localhost traffic..."

    # Apply to ALL traffic on localhost (lo0), IPv4 and IPv6
    # 'quick' ensures these rules are applied immediately when matched.
    local pf_rules=""
    # IPv4 localhost
    pf_rules+="dummynet in  quick on lo0 inet  all pipe $PIPE_NUM"$'\n'
    pf_rules+="dummynet out quick on lo0 inet  all pipe $PIPE_NUM"$'\n'
    # IPv6 localhost
    pf_rules+="dummynet in  quick on lo0 inet6 all pipe $PIPE_NUM"$'\n'
    pf_rules+="dummynet out quick on lo0 inet6 all pipe $PIPE_NUM"$'\n'

    # Apply rules to PF anchor (isolated from other rules); -q silences the -f warning
    echo "$pf_rules" | pfctl -q -a "$ANCHOR" -f -

    # Enable PF if not already enabled
    if ! pfctl -s info | grep -q "Status: Enabled"; then
        echo "Enabling packet filter..."
        # -E is reference-counted enable on macOS
        pfctl -E >/dev/null 2>&1 || pfctl -e >/dev/null 2>&1 || true
    fi

    echo "Network simulation started successfully!"
    echo ""
    echo "To adjust settings, run: $0 stop && sudo $0 start [new_params]"
    echo "To stop simulation, run: sudo $0 stop"
}

show_status() {
    echo "=== Network Simulation Status ==="
    echo ""

    # Check if PF is enabled
    echo "Packet Filter Status:"
    if pfctl -s info | grep -q "Status: Enabled"; then
        echo "  ✓ Enabled"
    else
        echo "  ✗ Disabled (simulation not active)"
        echo ""
        return
    fi
    echo ""

    # Show our dummynet rules in the anchor
    echo "Hypha dummynet rules (anchor: $ANCHOR):"
    if pfctl -a "$ANCHOR" -s dummynet 2>/dev/null | grep -q .; then
        pfctl -a "$ANCHOR" -s dummynet | sed 's/^/  /'
    else
        echo "  (no dummynet rules configured)"
    fi
    echo ""

    # Show dummynet pipe configuration and counters
    echo "Dummynet Pipe $PIPE_NUM:"
    if dnctl pipe "$PIPE_NUM" show 2>/dev/null | grep -q .; then
        dnctl pipe "$PIPE_NUM" show | sed 's/^/  /'
    else
        echo "  (pipe not configured)"
    fi
}

stop_simulation() {
    echo "Stopping network simulation..."

    # Flush rules from our anchor (only this anchor, not system rules)
    pfctl -q -a "$ANCHOR" -F all 2>/dev/null || true

    # Delete dummynet pipe
    dnctl pipe delete "$PIPE_NUM" 2>/dev/null || true

    echo "Network simulation stopped. Normal network conditions restored."
    echo ""
    echo "Note: PF remains enabled but localhost is no longer affected by this script."
}

# Main command dispatch
case "${1:-}" in
    start)
        check_root
        shift
        start_simulation "$@"
        ;;
    status)
        check_root
        show_status
        ;;
    stop)
        check_root
        stop_simulation
        ;;
    -h|--help|help)
        show_usage
        ;;
    *)
        show_usage
        exit 1
        ;;
esac
