#!/usr/bin/env bash
# setup.sh — Set up deskmic on a new machine
# Run from the repo root: ./scripts/setup.sh
#
# Prerequisites:
#   - Windows with WSL2
#   - Rust toolchain (cargo)
#   - .env file with secrets (copy from .env.example)

set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${REPO_DIR}/.env"
SCRIPTS_DIR="${REPO_DIR}/scripts"

echo "=== deskmic setup ==="

# 1. Check .env
if [[ ! -f "$ENV_FILE" ]]; then
    echo "ERROR: .env not found. Copy .env.example to .env and fill in your values."
    exit 1
fi
set -a; source "$ENV_FILE"; set +a
echo "[ok] .env loaded"

# 2. Build release binary
echo "Building deskmic..."
cd "$REPO_DIR"
cargo build --release
echo "[ok] Built target/release/deskmic.exe"

# 3. Install binary
INSTALL_DIR="${LOCALAPPDATA:-/mnt/c/Users/${USER}/AppData/Local}/deskmic"
if [[ "$INSTALL_DIR" == /mnt/* ]]; then
    true  # already a WSL path
else
    INSTALL_DIR=$(wslpath -u "$INSTALL_DIR" 2>/dev/null || echo "$INSTALL_DIR")
fi
mkdir -p "$INSTALL_DIR"
cp "$REPO_DIR/target/release/deskmic.exe" "$INSTALL_DIR/"
echo "[ok] Installed deskmic.exe to $INSTALL_DIR"

# 4. Copy .env next to binary for the .cmd script
cp "$ENV_FILE" "$INSTALL_DIR/.env"
echo "[ok] Copied .env to $INSTALL_DIR"

# 5. Copy scheduled task wrapper
cp "$SCRIPTS_DIR/deskmic-index-and-sync.cmd" "$INSTALL_DIR/"
echo "[ok] Copied deskmic-index-and-sync.cmd"

# 6. Install WSL wrapper scripts to ~/.local/bin
mkdir -p "$HOME/.local/bin"
cp "$SCRIPTS_DIR/deskmic-sync" "$HOME/.local/bin/deskmic-sync"
cp "$SCRIPTS_DIR/deskmicctl" "$HOME/.local/bin/deskmicctl"
chmod +x "$HOME/.local/bin/deskmic-sync" "$HOME/.local/bin/deskmicctl"
echo "[ok] Installed deskmic-sync and deskmicctl to ~/.local/bin"

# 7. Generate deskmic.toml from .env
TOML_FILE="$INSTALL_DIR/deskmic.toml"
if [[ ! -f "$TOML_FILE" ]]; then
    echo "Running deskmic setup wizard..."
    powershell.exe -Command "& '$(wslpath -w "$INSTALL_DIR/deskmic.exe")' setup"
fi
echo "[ok] Config at $TOML_FILE"

# 8. Create Windows scheduled tasks
echo "Creating scheduled tasks..."
WIN_INSTALL_DIR=$(wslpath -w "$INSTALL_DIR" 2>/dev/null || echo "$INSTALL_DIR")
powershell.exe -Command "
\$exe = '${WIN_INSTALL_DIR}\\deskmic.exe'
\$cmd = '${WIN_INSTALL_DIR}\\deskmic-index-and-sync.cmd'

# deskmic-reindex (daily at 4pm, runs index + blob push)
if (-not (Get-ScheduledTask -TaskName 'deskmic-reindex' -ErrorAction SilentlyContinue)) {
    \$action = New-ScheduledTaskAction -Execute \$cmd
    \$trigger = New-ScheduledTaskTrigger -Daily -At '4:00PM'
    Register-ScheduledTask -TaskName 'deskmic-reindex' -Action \$action -Trigger \$trigger -Description 'Index deskmic transcripts and push to blob'
    Write-Host '[ok] Created deskmic-reindex task'
} else {
    Write-Host '[skip] deskmic-reindex already exists'
}

# deskmic-daily-summary (daily at 6pm)
if (-not (Get-ScheduledTask -TaskName 'deskmic-daily-summary' -ErrorAction SilentlyContinue)) {
    \$action = New-ScheduledTaskAction -Execute \$exe -Argument 'summarize daily'
    \$trigger = New-ScheduledTaskTrigger -Daily -At '6:00PM'
    Register-ScheduledTask -TaskName 'deskmic-daily-summary' -Action \$action -Trigger \$trigger -Description 'Email daily deskmic summary'
    Write-Host '[ok] Created deskmic-daily-summary task'
} else {
    Write-Host '[skip] deskmic-daily-summary already exists'
}

# deskmic-weekly-summary (Fridays at 5pm)
if (-not (Get-ScheduledTask -TaskName 'deskmic-weekly-summary' -ErrorAction SilentlyContinue)) {
    \$action = New-ScheduledTaskAction -Execute \$exe -Argument 'summarize weekly'
    \$trigger = New-ScheduledTaskTrigger -Weekly -DaysOfWeek Friday -At '5:00PM'
    Register-ScheduledTask -TaskName 'deskmic-weekly-summary' -Action \$action -Trigger \$trigger -Description 'Email weekly deskmic summary'
    Write-Host '[ok] Created deskmic-weekly-summary task'
} else {
    Write-Host '[skip] deskmic-weekly-summary already exists'
}
"

# 9. Install to Windows startup
echo "Installing to Windows Startup..."
powershell.exe -Command "& '$(wslpath -w "$INSTALL_DIR/deskmic.exe")' install"
echo "[ok] Added to Windows Startup"

# 10. Pull existing data from blob (if available)
if [[ -n "${DESKMIC_BLOB_ACCOUNT:-}" ]]; then
    echo "Pulling existing data from blob..."
    deskmic-sync now || echo "[warn] Blob pull failed (container may be empty)"
fi

echo ""
echo "=== Setup complete ==="
echo ""
echo "Start deskmic:     deskmic.exe (double-click or from Startup)"
echo "Check status:      deskmicctl stats"
echo "Sync from blob:    deskmic-sync now"
echo "Push to blob:      deskmic-sync push"
