FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

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

# Non-root user — UID/GID match the host user so bind-mounted files stay correctly owned
ARG UID=1000
ARG GID=1000
RUN userdel -r ubuntu 2>/dev/null || true && \
    groupdel ubuntu 2>/dev/null || true && \
    groupadd -g "${GID}" sandbox && \
    useradd -m -u "${UID}" -g "${GID}" -s /bin/bash sandbox

COPY --chown=root:root entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

USER sandbox
WORKDIR /home/sandbox

# Create agtx dirs as sandbox user so named volume mounts inherit correct ownership
RUN mkdir -p \
    /home/sandbox/.config/agtx \
    /home/sandbox/.local/share/agtx \
    /home/sandbox/.claude

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["agtx"]
