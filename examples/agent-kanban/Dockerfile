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
    PATH=/usr/local/cargo/bin:$PATH

# Current git (the merge tests use `git replay --ref-action`, newer than distro git).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl gnupg software-properties-common \
 && add-apt-repository -y ppa:git-core/ppa \
 && apt-get update \
 && apt-get install -y --no-install-recommends git build-essential pkg-config python3 sqlite3 jq ripgrep less \
 && rm -rf /var/lib/apt/lists/*

# The agent user isn't root: let it write the cargo registry and git caches. Login shells
# (Codex runs commands with `bash -lc`) reset PATH in /etc/profile, so put cargo back there.
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --component clippy,rustfmt \
 && mkdir -p "$CARGO_HOME/registry" "$CARGO_HOME/git" \
 && chmod -R a+rwX "$CARGO_HOME" "$RUSTUP_HOME" \
 && echo 'export PATH="/usr/local/cargo/bin:$PATH"' > /etc/profile.d/cargo.sh
