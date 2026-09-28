# Worktree setup for emojicode (runs before every agent run, on the host or in the container).
# Configures build/ for whichever toolchain it runs on: the container sets LLVM_DIR, the host
# uses Homebrew's LLVM. A build dir configured for the other toolchain is recreated.
llvm_dir="${LLVM_DIR:-$(brew --prefix llvm)/lib/cmake/llvm}"
if [ -f build/CMakeCache.txt ] && ! grep -q "^LLVM_DIR:[A-Z]*=${llvm_dir}\$" build/CMakeCache.txt; then
  rm -rf build
fi
cmake -S . -B build -G Ninja -DLLVM_DIR="$llvm_dir" >/dev/null
