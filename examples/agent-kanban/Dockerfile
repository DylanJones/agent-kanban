# Toolchain image for agents working on agent-kanban itself. The source isn't copied in:
# agent-kanban mounts each run's worktree (and the repo's .git) into the container at run time.
# agent-kanban layers Node, the ACP adapters and a non-root `agent` user on top of this.
#
# Inside the container there's no Docker, so a test server an agent starts needs
# `serve --dangerously-allow-host-agents` to run agents at all (the container is the sandbox).
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive \
    LANG=C.UTF-8 \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    CARGO_TARGET_DIR=/tmp/cargo-target
# CARGO_TARGET_DIR keeps build output on the container's own disk: rustc intermittently fails
# ("can't find crate") when it writes to the macOS bind mount the worktree lives on. A clean
# build takes about 25 seconds.

# Current git (the merge tests use `git replay --ref-action`, newer than distro git).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl gnupg software-properties-common \
 && add-apt-repository -y ppa:git-core/ppa \
 && apt-get update \
 && apt-get install -y --no-install-recommends git build-essential pkg-config python3 sqlite3 jq ripgrep less \
 && rm -rf /var/lib/apt/lists/*

# Headless Chromium so agents can screenshot UI changes, e.g.
#   playwright screenshot --viewport-size=375,812 --color-scheme=dark http://127.0.0.1:7900/p/demo shot.png
# Node is needed here to install it (agent-kanban's overlay adds the same Node again).
COPY --from=node:24-slim /usr/local /usr/local
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
RUN npm install -g --no-fund --no-audit playwright@1.63.0 \
 && playwright install --with-deps --only-shell chromium \
 && apt-get install -y --no-install-recommends fonts-noto-color-emoji \
 && rm -rf /var/lib/apt/lists/* \
 && chmod -R a+rX "$PLAYWRIGHT_BROWSERS_PATH"

# The agent user isn't root: let it write the cargo registry and git caches. Login shells
# (Codex runs commands with `bash -lc`) reset PATH in /etc/profile, so put cargo back there.
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --component clippy,rustfmt \
 && mkdir -p "$CARGO_HOME/registry" "$CARGO_HOME/git" \
 && chmod -R a+rwX "$CARGO_HOME" "$RUSTUP_HOME" \
 && echo 'export PATH="/usr/local/cargo/bin:$PATH"' > /etc/profile.d/cargo.sh
