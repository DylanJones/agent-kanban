#!/bin/sh
# Run agent-kanban as a macOS LaunchAgent: starts at login and restarts if it exits.
#
#   scripts/service.sh install [bind]   # default bind 127.0.0.1:7878
#   scripts/service.sh restart          # after `cargo build --release`
#   scripts/service.sh status | logs | login-url | uninstall
#
# It runs target/release/agent-kanban from this checkout (override with AKB_BIN), so a rebuild
# plus `restart` deploys. Output goes to <data dir>/logs/server.log, readable only by you
# (it contains the login link).
set -eu

LABEL=dev.agent-kanban
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${AKB_BIN:-$ROOT/target/release/agent-kanban}
DATA=${AKB_DATA_DIR:-$HOME/.agent-kanban}
PLIST=$HOME/Library/LaunchAgents/$LABEL.plist
LOG=$DATA/logs/server.log
DOMAIN=gui/$(id -u)
# launchd starts jobs with a bare PATH; agents need git, node/npx, docker and gh.
SVC_PATH=/opt/homebrew/bin:/usr/local/bin:$HOME/.local/bin:$HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin

loaded() { launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; }

case ${1:-} in
install)
    BIND=${2:-127.0.0.1:7878}
    [ -x "$BIN" ] || { echo "no binary at $BIN; run: cargo build --release" >&2; exit 1; }
    mkdir -p "$DATA/logs" "$(dirname "$PLIST")"
    chmod 700 "$DATA/logs"
    cat >"$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN</string>
    <string>serve</string>
    <string>--bind</string>
    <string>$BIND</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>$SVC_PATH</string>
    <key>AKB_DATA_DIR</key><string>$DATA</string>
    <key>AKB_SUPERVISED</key><string>launchd</string>
  </dict>
  <key>WorkingDirectory</key><string>$DATA</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>Umask</key><integer>63</integer>
  <key>StandardOutPath</key><string>$LOG</string>
  <key>StandardErrorPath</key><string>$LOG</string>
</dict>
</plist>
EOF
    touch "$LOG" && chmod 600 "$LOG"
    if loaded; then launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true; sleep 1; fi
    launchctl bootstrap "$DOMAIN" "$PLIST"
    echo "installed $LABEL (bind $BIND); log: $LOG"
    echo "login link: $0 login-url"
    ;;
uninstall)
    loaded && launchctl bootout "$DOMAIN/$LABEL" || true
    rm -f "$PLIST"
    echo "removed $LABEL"
    ;;
restart) launchctl kickstart -k "$DOMAIN/$LABEL" ;;
status) launchctl print "$DOMAIN/$LABEL" | grep -E '^\s*(state|pid|last exit code|program) ' ;;
logs) tail -n 50 -f "$LOG" ;;
login-url) AKB_DATA_DIR=$DATA "$BIN" login-url ;;
*) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
