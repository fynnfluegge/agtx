#!/bin/bash
set -e

# Copy host ~/.claude credentials into the writable sandbox home at startup.
# The host directory is mounted read-only at /claude-host so Claude Code can
# read credentials without the container ever writing back to the host.
if [ -d /claude-host ]; then
    cp -rp /claude-host/. /home/sandbox/.claude/ 2>/dev/null || true
fi

# Pre-accept the bypass permissions prompt — appropriate because we are inside
# an isolated container with no access to the host filesystem beyond the
# project directory.
settings=/home/sandbox/.claude/settings.json
if [ -f "$settings" ]; then
    node -e "
        const fs = require('fs');
        const s = JSON.parse(fs.readFileSync('$settings', 'utf8'));
        s.skipDangerousModePermissionPrompt = true;
        fs.writeFileSync('$settings', JSON.stringify(s, null, 2));
    "
else
    echo '{"skipDangerousModePermissionPrompt":true}' > "$settings"
fi

exec "$@"
