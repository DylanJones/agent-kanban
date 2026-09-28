# Toolchain image for agents working on emojicode. The source isn't copied in: agent-kanban
# mounts each run's worktree (and the repo's .git) into the container at run time.
# Mirrors CI (.github/workflows/ci.yml) and docker/clang: Ubuntu 24.04 + Clang/LLVM from apt.llvm.org.
# agent-kanban layers Node, the ACP adapters and a non-root `agent` user on top of this.
FROM ubuntu:24.04

ARG LLVM_VERSION=23
ENV DEBIAN_FRONTEND=noninteractive \
    PYTHONIOENCODING=utf8 \
    LANG=C.UTF-8 \
    CC=clang-${LLVM_VERSION} \
    CXX=clang++-${LLVM_VERSION} \
    LLVM_DIR=/usr/lib/llvm-${LLVM_VERSION}/lib/cmake/llvm \
    CMAKE_C_COMPILER_LAUNCHER=ccache \
    CMAKE_CXX_COMPILER_LAUNCHER=ccache \
    EMOJICODE_TEST_JOBS=4

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates wget gnupg \
 && wget -qO /etc/apt/trusted.gpg.d/apt.llvm.org.asc https://apt.llvm.org/llvm-snapshot.gpg.key \
 && echo "deb http://apt.llvm.org/noble/ llvm-toolchain-noble-${LLVM_VERSION} main" > /etc/apt/sources.list.d/llvm.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      clang-${LLVM_VERSION} lld-${LLVM_VERSION} llvm-${LLVM_VERSION}-dev \
      cmake ninja-build ccache python3 rsync zlib1g-dev libzstd-dev \
      git curl jq less file ripgrep xz-utils \
 && rm -rf /var/lib/apt/lists/*

# Optional: the tree-sitter CLI enables the editor-grammar part of `ninja grammar`.
ARG TARGETARCH
RUN arch=$([ "$TARGETARCH" = "arm64" ] && echo arm64 || echo x64) \
 && (wget -qO- "https://github.com/tree-sitter/tree-sitter/releases/latest/download/tree-sitter-linux-${arch}.gz" \
       | gunzip > /usr/local/bin/tree-sitter \
     && chmod +x /usr/local/bin/tree-sitter && tree-sitter --version) \
 || rm -f /usr/local/bin/tree-sitter
