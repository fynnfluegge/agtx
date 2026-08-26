#!/bin/bash
set -e

# Copy host ~/.claude credentials into the writable sandbox home at startup.
# The host directory is mounted read-only at /claude-host so Claude Code can
# read credentials without the container ever writing back to the host.
if [ -d /claude-host ]; then
    cp -rp /claude-host/. /home/sandbox/.claude/ 2>/dev/null || true
fi

# Copy the host's ~/.claude.json snapshot: onboarding state (so Claude does not
# open on its theme picker) plus the per-directory trust map. Mounted read-only,
# copied so the container can write to it.
if [ -f /claude-host.json ]; then
    cp /claude-host.json /home/sandbox/.claude.json 2>/dev/null || true
    chmod u+w /home/sandbox/.claude.json 2>/dev/null || true
fi

# Trust the workspace for Claude. Trust records are path-keyed and the project is
# mounted at a path the host never used, so the copied map above never matches it.
# Appropriate here for the same reason as the bypass pre-accept: an isolated,
# disposable container holding nothing but the project.
node -e "
    const fs = require('fs');
    const p = '/home/sandbox/.claude.json';
    let d = {};
    try { d = JSON.parse(fs.readFileSync(p, 'utf8')); } catch (e) {}
    d.projects = d.projects || {};
    d.projects['/home/sandbox/workspace'] = d.projects['/home/sandbox/workspace'] || {};
    d.projects['/home/sandbox/workspace'].hasTrustDialogAccepted = true;
    fs.writeFileSync(p, JSON.stringify(d, null, 2));
"

# Drop empty auth keys from the copied settings.json. That `env` block outranks
# process env, so an `ANTHROPIC_AUTH_TOKEN: ""` left over from a proxy setup
# shadows *both* the Keychain credentials planted above and a real
# ANTHROPIC_API_KEY — and the agent reports "Not logged in" with valid auth
# available. The host file is untouched; only the container's copy is edited.
settings_env=/home/sandbox/.claude/settings.json
if [ -f "$settings_env" ]; then
    node -e "
        const fs = require('fs');
        const p = '$settings_env';
        let d;
        try { d = JSON.parse(fs.readFileSync(p, 'utf8')); } catch (e) { process.exit(0); }
        const env = d.env;
        if (env && typeof env === 'object') {
            const dropped = Object.keys(env).filter((k) => env[k] === '');
            for (const k of dropped) delete env[k];
            if (dropped.length) {
                fs.writeFileSync(p, JSON.stringify(d, null, 2));
                console.log('dropped empty env keys: ' + dropped.join(', '));
            }
        }
    "
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

# agtx no longer answers trust prompts by reading the pane (`auto_trust`, default
# off), so inside the sandbox it is turned on explicitly. Same reasoning as the
# bypass pre-accept above: the container is disposable, holds nothing but the
# project, and there is no human at the board to answer a prompt.
mkdir -p /home/sandbox/.config/agtx
agtx_config=/home/sandbox/.config/agtx/config.toml
if [ -f "$agtx_config" ]; then
    grep -q '^auto_trust' "$agtx_config" || echo 'auto_trust = true' >> "$agtx_config"
else
    echo 'auto_trust = true' > "$agtx_config"
fi

exec "$@"
