FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
# Ubuntu's OpenSSL auto-enables its FIPS provider when the host kernel is in
# FIPS mode, but this image ships no FIPS provider module, so every digest
# fails there (e.g. the ca-certificates postinst). No-op on non-FIPS hosts.
ENV OPENSSL_FORCE_FIPS_MODE=0

ARG NODE_MAJOR=22

# System dependencies + Node.js via NodeSource (handles arch automatically)
RUN apt-get update && apt-get install -y \
    curl git tmux ca-certificates \
    && curl -fsSL https://deb.nodesource.com/setup_${NODE_MAJOR}.x | bash - \
    && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code globally
RUN npm install -g @anthropic-ai/claude-code

# Install agtx
RUN curl -fsSL https://raw.githubusercontent.com/fynnfluegge/agtx/main/install.sh \
    | AGTX_INSTALL_DIR=/usr/local/bin bash

# The container's agtx is baked into the image; a release-check notice inside it
# would point at an update the user cannot apply from here, and every container
# start would spend a request against the shared-IP GitHub rate limit.
ENV AGTX_NO_UPDATE_CHECK=1

# Non-root user — UID/GID match the host user so bind-mounted files stay correctly owned
ARG UID=1000
ARG GID=1000
# `groupadd -g` fails (exit 4) when the GID is already taken, which it is for
# every macOS user: `id -g` is 20 there, and GID 20 is `dialout` in Ubuntu 24.04.
# Reuse whatever group already owns the GID instead — the point is only that
# bind-mounted files come out owned by the host user, and the group's *name* is
# irrelevant to that.
RUN userdel -r ubuntu 2>/dev/null || true && \
    groupdel ubuntu 2>/dev/null || true && \
    groupadd -g "${GID}" sandbox 2>/dev/null || true && \
    useradd -m -u "${UID}" -g "${GID}" -s /bin/bash sandbox

COPY --chown=root:root entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Create agtx dirs so named volume mounts inherit correct ownership.
# Group-writable, not user-owned: rootless builds can't setuid to huge host UIDs.
RUN mkdir -p /home/sandbox/.config/agtx /home/sandbox/.claude && \
    chgrp -R "${GID}" /home/sandbox && chmod -R g+w /home/sandbox

USER sandbox
WORKDIR /home/sandbox

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["agtx"]
