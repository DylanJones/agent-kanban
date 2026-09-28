//! Rebuilding and restarting the server from the app (issue #12): the web UI and server binary
//! are rebuilt on the host from the checkout this binary was compiled from, and the process then
//! hands off to the rebuilt one — either by re-executing itself, or by exiting for a supervisor
//! (the `scripts/service.sh` LaunchAgent, which has `KeepAlive`) to relaunch it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::AppState;
use crate::config::Config;

/// The only branch a rebuild ever builds from. `Config::repo_dir` is this server's own checkout
/// (not a generic per-project setting), and on the host it's always `main`.
const BASE_BRANCH: &str = "main";

/// The commit and time this binary was compiled from (embedded by `build.rs`).
pub fn build_sha() -> &'static str {
    env!("AKB_BUILD_SHA")
}
pub fn build_time() -> &'static str {
    env!("AKB_BUILD_TIME")
}

/// A random id generated once per process start, so the UI can tell that the server actually
/// restarted even when a rebuild produces the same `build_sha` (no new commits, just a restart).
pub fn instance_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(crate::auth::random_token)
}

/// One command of the build pipeline (see `build_steps`).
#[derive(Debug, Clone)]
pub struct BuildStep {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// True only for the real `cargo build --release` step: its stdout is Cargo's own
    /// `--message-format=json` stream (see `build_steps`), which `run_step` parses instead of
    /// logging verbatim, to find the executable Cargo actually produced.
    pub cargo_json: bool,
}

/// Coordinates an in-progress or pending rebuild/restart, for the server's whole lifetime.
pub struct DeployState {
    /// Set for the duration of a "restart now" rebuild, and while a "restart once idle" one is
    /// waiting for active runs to finish; checked by the scheduler so it stops dispatching new
    /// runs. Independent of (and doesn't change) the `scheduler_enabled` setting.
    pub dispatch_paused: AtomicBool,
    /// Reserves the whole build-through-restart pipeline to one deployment at a time: set by the
    /// `rebuild` endpoint on admission, cleared only if the build fails (a success leads to a
    /// restart, so the process doesn't stick around to need it cleared).
    pub deploying: AtomicBool,
    /// Serializes "start a new run" against "decide there are zero active runs and it's safe to
    /// restart", so a run can't be admitted in the gap between those two checks (see
    /// `orchestrator::scheduler::start_run` and `schedule_restart`).
    pub dispatch_gate: tokio::sync::Mutex<()>,
    /// Set once a successful build wants the process to restart. `main` waits on `notify`, then
    /// checks this to decide whether to actually restart (vs. a plain Ctrl-C shutdown).
    pub restart_requested: AtomicBool,
    pub notify: tokio::sync::Notify,
    /// Where the most recent successful build actually placed the executable (resolved from
    /// Cargo's own `--message-format=json` output, not guessed from `CARGO_TARGET_DIR`
    /// conventions — see `run_build_job`). Read by `perform_restart`, which runs in a later,
    /// independent call after the build has already returned.
    pub last_build_artifact: std::sync::Mutex<Option<PathBuf>>,
    /// Explanation for the most recent deployment failure, set by `fail_deploy` and cleared by
    /// `try_begin_deploy` on the next attempt. Exposed on `BuildStatus` so a page awaiting a
    /// deploy can learn it failed and stop waiting even when the DB writes that would normally
    /// record that on the job row (its terminal status, or even its log) themselves fail — an
    /// in-memory field can't fail to persist the way those can. See thread #122 on issue #12.
    pub deploy_error: std::sync::Mutex<Option<String>>,
}

impl Default for DeployState {
    fn default() -> Self {
        DeployState {
            dispatch_paused: AtomicBool::new(false),
            deploying: AtomicBool::new(false),
            dispatch_gate: tokio::sync::Mutex::new(()),
            restart_requested: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
            last_build_artifact: std::sync::Mutex::new(None),
            deploy_error: std::sync::Mutex::new(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestartMode {
    /// Restart immediately; active runs are interrupted and resume after the restart.
    Now,
    /// Stop dispatching new runs and restart once the active ones finish.
    Drain,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BuildStatus {
    /// Commit this running server was built from ("unknown" if it couldn't be determined when
    /// compiling, e.g. no `.git` in the build context).
    pub build_sha: String,
    pub build_time: String,
    /// Random per-process id: changes across a restart even if `build_sha` doesn't (e.g. a
    /// restart with no new commits), so the UI can tell the server actually came back.
    pub instance_id: String,
    /// Tip of the checkout's local `main` branch (what a rebuild is meant to build), not
    /// necessarily what's currently checked out there — see `on_main`.
    pub head_sha: Option<String>,
    pub head_summary: Option<String>,
    /// Commits `build_sha` is behind `main`; `null` if unknown (e.g. `build_sha` isn't reachable
    /// from `main` — a shallow clone, or history that was rewritten).
    pub commits_behind: Option<i64>,
    /// The checkout has uncommitted changes.
    pub dirty: bool,
    /// The checkout's `HEAD` is `main` (as opposed to a detached HEAD or another branch); a
    /// rebuild always builds whatever is actually checked out, so `commits_behind` and a rebuild
    /// button are only meaningful when this is true.
    pub on_main: bool,
    pub dispatch_paused: bool,
    /// A build succeeded and the process is about to restart, or (in drain mode) is waiting to.
    pub restart_pending: bool,
    pub active_runs: i64,
    /// Set for as long as the most recent deployment attempt's failure hasn't been superseded by
    /// a new one (`try_begin_deploy` clears it on admission). A page awaiting a deploy should
    /// treat this as a failure and stop waiting even if the triggering job's own row never made it
    /// to `status: "failed"` (see `DeployState::deploy_error`).
    pub deploy_error: Option<String>,
}

pub async fn status(app: &AppState) -> BuildStatus {
    let repo = &app.config.repo_dir;
    let main_sha = crate::git::branch_sha(repo, BASE_BRANCH).await;
    let on_main = crate::git::run(repo, &["symbolic-ref", "-q", "--short", "HEAD"]).await.ok().as_deref() == Some(BASE_BRANCH);
    let head_summary = match &main_sha {
        Some(h) => crate::git::run(repo, &["log", "-1", "--format=%s", h]).await.ok(),
        None => None,
    };
    let commits_behind = match &main_sha {
        Some(m) if build_sha() != "unknown" && crate::git::is_ancestor(repo, build_sha(), m).await => {
            crate::git::commits_ahead(repo, build_sha(), m).await.ok().map(|n| n as i64)
        }
        _ => None,
    };
    let dirty = crate::git::run(repo, &["status", "--porcelain"]).await.is_ok_and(|s| !s.trim().is_empty());
    let active_runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE status IN ('queued','preparing','running')")
        .fetch_one(&app.db)
        .await
        .unwrap_or(0);
    BuildStatus {
        build_sha: build_sha().to_string(),
        build_time: build_time().to_string(),
        instance_id: instance_id().to_string(),
        head_sha: main_sha,
        head_summary,
        commits_behind,
        dirty,
        on_main,
        dispatch_paused: app.deploy.dispatch_paused.load(Ordering::Relaxed),
        restart_pending: app.deploy.restart_requested.load(Ordering::Relaxed),
        active_runs,
        deploy_error: app.deploy.deploy_error.lock().unwrap().clone(),
    }
}

const LOCK_MARKER: &str = "node_modules/.akb-package-lock.sha256";

/// Whether `npm ci` is needed: no `node_modules` yet, or `package-lock.json` changed since the
/// last `npm ci` (tracked with a marker file — re-running `npm ci` on every rebuild is slow).
fn web_deps_changed(web: &Path) -> bool {
    if !web.join("node_modules").is_dir() {
        return true;
    }
    let Ok(lock) = std::fs::read(web.join("package-lock.json")) else { return true };
    let hash = hex::encode(Sha256::digest(&lock));
    std::fs::read_to_string(web.join(LOCK_MARKER)).map(|s| s.trim() != hash).unwrap_or(true)
}

fn mark_web_deps_current(web: &Path) {
    if let Ok(lock) = std::fs::read(web.join("package-lock.json")) {
        let hash = hex::encode(Sha256::digest(&lock));
        let _ = std::fs::write(web.join(LOCK_MARKER), hash);
    }
}

/// The real build steps: `npm ci` only when dependencies changed, then `npm run build`, then
/// `cargo build --release`. Replaced wholesale by `Config::build_steps_override` in tests, so the
/// rebuild flow can be exercised without a real (slow) compile.
fn build_steps(config: &Config) -> Vec<BuildStep> {
    if let Some(steps) = &config.build_steps_override {
        return steps.clone();
    }
    let repo = &config.repo_dir;
    let web = repo.join("web");
    let mut steps = Vec::new();
    if web_deps_changed(&web) {
        steps.push(BuildStep { label: "npm ci".into(), program: "npm".into(), args: vec!["ci".into()], cwd: web.clone(), cargo_json: false });
    }
    steps.push(BuildStep {
        label: "npm run build".into(),
        program: "npm".into(),
        args: vec!["run".into(), "build".into()],
        cwd: web.clone(),
        cargo_json: false,
    });
    steps.push(BuildStep {
        label: "cargo build --release".into(),
        program: "cargo".into(),
        // Plain (non-ANSI) JSON: `rendered` diagnostics stay readable in the plain-text log the
        // UI streams, and `run_step` parses this stream to find the executable Cargo actually
        // produced (see thread #101 — a guessed `<target-dir>/release/<bin>` path can't account
        // for a configured `.cargo/config.toml` `target-dir` or target triple).
        args: vec!["build".into(), "--release".into(), "--message-format=json".into()],
        cwd: repo.clone(),
        cargo_json: true,
    });
    steps
}

/// One line of Cargo's `--message-format=json` output (see `run_step`). Only the two reasons we
/// act on are modeled; everything else (`build-script-executed`, `build-finished`, …) is ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum CargoMessage {
    CompilerArtifact { target: CargoTarget, executable: Option<PathBuf> },
    CompilerMessage { message: CargoDiagnostic },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoDiagnostic {
    rendered: Option<String>,
}

/// Handles one line of Cargo's `--message-format=json` stdout: logs a human-readable form and
/// returns the executable path if the line reports our own `bin` target's artifact. Kept separate
/// from `run_step` so this parsing (the fix for thread #101 on issue #12: a guessed
/// `<target-dir>/release/<bin>` path can't account for a configured `.cargo/config.toml`
/// `target-dir` or target triple) is unit-testable without spawning a real `cargo build`.
fn handle_cargo_json_line(line: &str, log: &mut (dyn FnMut(String) + Send)) -> Option<PathBuf> {
    match serde_json::from_str::<CargoMessage>(line) {
        Ok(CargoMessage::CompilerArtifact { target, executable: Some(exe) }) if target.kind.iter().any(|k| k == "bin") && target.name == env!("CARGO_PKG_NAME") => Some(exe),
        Ok(CargoMessage::CompilerMessage { message: CargoDiagnostic { rendered: Some(r) } }) => {
            log(r.trim_end().to_string());
            None
        }
        Ok(_) => None,
        // Cargo's JSON stream is line-delimited and shouldn't contain anything else, but don't
        // silently swallow a line that failed to parse.
        Err(_) => {
            log(line.to_string());
            None
        }
    }
}

/// Runs one step, streaming both stdout and stderr to `log` as they arrive. Both streams are
/// drained independently until each hits EOF before waiting on the child: stopping at the first
/// EOF (as a naive `select!` would) can drop output still coming on the other stream — including
/// the compiler diagnostics a failed `cargo build` writes to stderr — or even deadlock a child
/// that closes one stream while still writing enough to the other to fill its pipe buffer.
///
/// For `step.cargo_json`, stdout is Cargo's own JSON message stream rather than plain text:
/// compiler diagnostics are logged via their pre-rendered plain-text form, and the executable
/// path Cargo reports for our own `bin` target is returned instead of guessed from a target-dir
/// convention (see thread #101 on issue #12). Stderr (Cargo's own "Compiling .../Finished ..."
/// progress and fatal errors) is logged verbatim either way.
async fn run_step(step: &BuildStep, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<Option<PathBuf>> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    log(format!("$ {} {} (in {})", step.program, step.args.join(" "), step.cwd.display()));
    let mut child = tokio::process::Command::new(&step.program)
        .args(&step.args)
        .current_dir(&step.cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawning {}: {e}", step.program))?;
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut out_done = false;
    let mut err_done = false;
    let mut artifact = None;
    while !out_done || !err_done {
        tokio::select! {
            l = out.next_line(), if !out_done => match l? {
                Some(l) if step.cargo_json => {
                    if let Some(exe) = handle_cargo_json_line(&l, log) {
                        artifact = Some(exe);
                    }
                }
                Some(l) => log(l),
                None => out_done = true,
            },
            l = err.next_line(), if !err_done => match l? {
                Some(l) => log(l),
                None => err_done = true,
            },
        }
    }
    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("{} failed ({status})", step.label);
    }
    Ok(artifact)
}

/// Refuses to build anything but `main`: `Config::repo_dir` is a single shared checkout (not a
/// per-run worktree), so if it's on a detached HEAD or another branch, a build would silently
/// deploy that instead of the `main` the status page shows commits behind.
async fn check_build_source(repo: &Path, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<()> {
    let branch = crate::git::run(repo, &["symbolic-ref", "-q", "--short", "HEAD"]).await.ok();
    if branch.as_deref() != Some(BASE_BRANCH) {
        anyhow::bail!(
            "checkout at {} is on {} instead of `{BASE_BRANCH}`; switch it to `{BASE_BRANCH}` before rebuilding",
            repo.display(),
            branch.as_deref().unwrap_or("a detached HEAD")
        );
    }
    if let Some(sha) = crate::git::branch_sha(repo, BASE_BRANCH).await {
        log(format!("building {BASE_BRANCH} at {sha}"));
    }
    Ok(())
}

/// Runs the build (see `build_steps`) and, only on success, arranges the restart. Invoked from
/// the `server.build` job (`jobs::dispatch`); a failed build leaves the running server untouched.
/// The actual restart is requested by `jobs::run_job`, only once it has durably recorded this job
/// as succeeded — not from here, so a crash or restart between "build succeeded" and "job row
/// updated" can't make `spawn_worker` requeue (and re-run) an already-successful build on the next
/// startup.
pub async fn run_build_job(app: &AppState, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<()> {
    if let Err(e) = check_build_source(&app.config.repo_dir, log).await {
        fail_deploy(app, &e.to_string());
        return Err(e);
    }
    let main_before = crate::git::branch_sha(&app.config.repo_dir, BASE_BRANCH).await;
    let steps = build_steps(&app.config);
    let mut cargo_artifact = None;
    for step in &steps {
        match run_step(step, log).await {
            Ok(exe) => cargo_artifact = cargo_artifact.or(exe),
            Err(e) => {
                fail_deploy(app, &e.to_string());
                return Err(e);
            }
        }
    }
    // A real `cargo build --release` always reports a `compiler-artifact` message for our own
    // `bin` target (even when it's already fresh/cached — Cargo still reports what's there), so a
    // real build with nothing captured means something about the JSON stream didn't match what
    // was expected; guessing a path at that point is exactly the bug this replaced (thread #101).
    // Only `Config::build_steps_override` (tests, with no real `cargo build` to parse JSON from)
    // falls back to the guessed `cargo_output_path`.
    let cargo_output = match (cargo_artifact, &app.config.build_steps_override) {
        (Some(exe), _) => exe,
        (None, Some(_)) => cargo_output_path(&app.config),
        (None, None) => {
            let msg = format!("cargo build succeeded but never reported an executable for `{}`; can't determine what to restart into", env!("CARGO_PKG_NAME"));
            fail_deploy(app, &msg);
            anyhow::bail!("{msg}");
        }
    };
    if app.config.build_steps_override.is_none() {
        mark_web_deps_current(&app.config.repo_dir.join("web"));
    }
    let main_after = crate::git::branch_sha(&app.config.repo_dir, BASE_BRANCH).await;
    if main_before != main_after {
        log(format!(
            "note: `{BASE_BRANCH}` advanced from {} to {} while building; the new binary reflects the checkout as it was built, not necessarily this latest tip",
            main_before.as_deref().unwrap_or("unknown"),
            main_after.as_deref().unwrap_or("unknown"),
        ));
    }
    *app.deploy.last_build_artifact.lock().unwrap() = Some(cargo_output.clone());
    if let Err(e) = install_build_artifact(&app.config, &cargo_output) {
        fail_deploy(app, &e.to_string());
        return Err(e);
    }
    let artifact = build_artifact_path(&app.config, Some(&cargo_output));
    if !artifact.is_file() {
        let msg = format!("build succeeded but no artifact was found at {} to restart into", artifact.display());
        fail_deploy(app, &msg);
        anyhow::bail!("{msg}");
    }
    log("build succeeded".into());
    Ok(())
}

/// After a successful build, copies the binary Cargo just produced (`src`, resolved by
/// `run_build_job` from Cargo's own JSON output — see thread #101) to the configured deployment
/// destination (`Config::restart_artifact`, e.g. `AKB_BIN`), if one is set and differs from `src`
/// — Cargo has no idea that destination exists and never writes there itself, so without this
/// step a configured `AKB_BIN` would keep pointing at whatever bytes happened to already be there
/// (e.g. a previous, now-stale deployment) rather than the build that just ran. Writes to a
/// sibling temp file first and renames it over the destination, so nothing can ever observe (or
/// restart into) a partially-written binary.
fn install_build_artifact(config: &Config, src: &Path) -> anyhow::Result<()> {
    let Some(dest) = &config.restart_artifact else { return Ok(()) };
    if paths_match(src, dest) {
        return Ok(());
    }
    let mut tmp = dest.clone().into_os_string();
    tmp.push(".new");
    let tmp = PathBuf::from(tmp);
    std::fs::copy(src, &tmp).with_context(|| format!("installing build artifact: copying {} to {}", src.display(), tmp.display()))?;
    std::fs::rename(&tmp, dest).with_context(|| format!("installing build artifact: renaming {} to {}", tmp.display(), dest.display()))?;
    Ok(())
}

/// Reverses `try_begin_deploy` after a failed build: dispatch resumes, and another rebuild can be
/// requested. Also called from `jobs::run_job` when a build succeeded but its result couldn't be
/// durably recorded (see the doc comment on `schedule_restart`'s caller there): the requeued job
/// will re-acquire these guards for itself on the next startup, so nothing is lost by releasing
/// them here rather than leaving the server looking permanently stuck mid-deploy. `reason` is
/// recorded on `DeployState::deploy_error` (and so `BuildStatus`) regardless of whether it was
/// also recorded on the job row, so a page awaiting this deploy can always learn it failed.
pub(crate) fn fail_deploy(app: &AppState, reason: &str) {
    app.deploy.dispatch_paused.store(false, Ordering::Relaxed);
    app.deploy.deploying.store(false, Ordering::Relaxed);
    *app.deploy.deploy_error.lock().unwrap() = Some(reason.to_string());
}

/// Reserves the deployment pipeline for one caller at a time. Returns `false` if a build, or a
/// wait-for-idle-then-restart, is already in progress.
pub fn try_begin_deploy(app: &AppState) -> bool {
    let began = app.deploy.deploying.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok();
    if began {
        *app.deploy.deploy_error.lock().unwrap() = None;
    }
    began
}

/// Called for a "drain" restart before the build even starts, so no new runs are dispatched while
/// it builds and waits for the active ones to finish. Takes the same gate `start_run` checks
/// under, so a run that's already mid-admission can't land after this point believing it read
/// `dispatch_paused == false`.
pub async fn begin_drain(app: &AppState) {
    let _guard = app.deploy.dispatch_gate.lock().await;
    app.deploy.dispatch_paused.store(true, Ordering::Relaxed);
}

/// Called once `jobs::run_job` has durably recorded the `server.build` job as succeeded.
pub(crate) fn schedule_restart(app: AppState, mode: RestartMode) {
    match mode {
        RestartMode::Now => request_restart(&app),
        RestartMode::Drain => {
            tokio::spawn(async move {
                loop {
                    let guard = app.deploy.dispatch_gate.lock().await;
                    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE status IN ('queued','preparing','running')")
                        .fetch_one(&app.db)
                        .await
                        .unwrap_or(0);
                    if active == 0 {
                        request_restart(&app);
                        return;
                    }
                    drop(guard);
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            });
        }
    }
}

fn request_restart(app: &AppState) {
    app.deploy.restart_requested.store(true, Ordering::Relaxed);
    // Wakes both the axum graceful-shutdown trigger and main's bound-shutdown timer (see main.rs).
    app.deploy.notify.notify_waiters();
}

pub fn should_restart(app: &AppState) -> bool {
    app.deploy.restart_requested.load(Ordering::Relaxed)
}

/// How the running process should hand off to the rebuilt binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartKind {
    ExitForSupervisor,
    ExecSelf,
}

/// Pure decision, so it's testable without a real LaunchAgent: `scripts/service.sh` sets
/// `AKB_SUPERVISED=launchd` in the plist it installs. But launchd always relaunches the same
/// on-disk path it was configured with — the path this process itself was started from
/// (`running_exe`) — so exiting only helps if that's also where the build just placed the new
/// binary (`artifact`); otherwise (a `cargo run` dev binary, a copied deployment, `AKB_BIN`
/// pointing elsewhere, …) launchd would just relaunch the stale one, and this process must exec
/// the artifact directly instead.
pub fn restart_kind(supervised_env: Option<&str>, running_exe: Option<&Path>, artifact: &Path) -> RestartKind {
    let relaunches_artifact = running_exe.is_some_and(|e| paths_match(e, artifact));
    if supervised_env == Some("launchd") && relaunches_artifact { RestartKind::ExitForSupervisor } else { RestartKind::ExecSelf }
}

fn paths_match(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

/// Where `cargo build --release` itself writes the binary: `<CARGO_TARGET_DIR or
/// repo_dir/target>/release/<bin name>`. A relative `CARGO_TARGET_DIR` is resolved against
/// `repo_dir`, since that's the directory the `cargo build` step actually runs in (see
/// `build_steps`), not necessarily this process's own cwd. Doesn't account for Cargo
/// config-file overrides (`.cargo/config.toml`'s `build.target-dir`, or a configured build target
/// triple) — `AKB_REPO_DIR`/`AKB_BIN` are the escape hatch for those.
fn cargo_output_path(config: &Config) -> PathBuf {
    let target = match &config.cargo_target_dir {
        Some(t) if t.is_absolute() => t.clone(),
        Some(t) => config.repo_dir.join(t),
        None => config.repo_dir.join("target"),
    };
    target.join("release").join(env!("CARGO_PKG_NAME"))
}

/// Where a rebuild restarts into: `Config::restart_artifact` (an explicit deployment destination,
/// e.g. `AKB_BIN`, or the one tests install) if set, else wherever `cargo build --release` itself
/// wrote the binary (`cargo_output_path`). When an explicit destination is configured,
/// `install_build_artifact` copies the freshly built binary there after every successful build —
/// Cargo has no idea that destination exists, so nothing else would ever put a new binary there.
/// `resolved_cargo_output`, when available, is what `run_build_job` actually found Cargo produce
/// (from its JSON output) for the build in progress; falls back to the guessed `cargo_output_path`
/// only when that isn't available (`perform_restart`, run from a fresh call with no build of its
/// own in progress, reads it back from `DeployState::last_build_artifact` instead).
pub fn build_artifact_path(config: &Config, resolved_cargo_output: Option<&Path>) -> PathBuf {
    config.restart_artifact.clone().unwrap_or_else(|| resolved_cargo_output.map(Path::to_path_buf).unwrap_or_else(|| cargo_output_path(config)))
}

/// Builds the command that hands off to the rebuilt binary (the `RestartKind::ExecSelf` case).
/// Kept separate from `perform_restart` so a test can run it as an ordinary child process (and
/// check it actually launched the intended artifact) instead of exec'ing over the test binary.
pub fn exec_self_command(artifact: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(artifact);
    cmd.args(std::env::args().skip(1));
    cmd
}

/// Performs the restart decided by `restart_kind`. Never returns on success (exits, or re-execs,
/// which replaces this process); exits with an error if re-exec itself fails to start.
pub fn perform_restart(app: &AppState) -> ! {
    let resolved = app.deploy.last_build_artifact.lock().unwrap().clone();
    let config = &app.config;
    let artifact = build_artifact_path(config, resolved.as_deref());
    match restart_kind(std::env::var("AKB_SUPERVISED").ok().as_deref(), config.running_exe.as_deref(), &artifact) {
        RestartKind::ExitForSupervisor => {
            tracing::info!("restarting: exiting for the supervisor to relaunch the rebuilt binary");
            std::process::exit(0);
        }
        RestartKind::ExecSelf => {
            use std::os::unix::process::CommandExt;
            if !artifact.is_file() {
                tracing::error!("restart: no build artifact at {}", artifact.display());
                std::process::exit(1);
            }
            tracing::info!("restarting: exec {}", artifact.display());
            let err = exec_self_command(&artifact).exec();
            tracing::error!("restart: exec failed: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn handle_cargo_json_line_captures_the_bin_targets_executable() {
        // A real line captured from `cargo build --release --message-format=json` for this crate,
        // plus a `lib`-target artifact line (no executable) that must be ignored, and a dependency's
        // own `bin` target (different name) that must also be ignored — only our binary counts.
        let mut logged = Vec::new();
        let mut log = |s: String| logged.push(s);
        let lib_line = r#"{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"agent_kanban"},"executable":null}"#;
        let other_bin_line = r#"{"reason":"compiler-artifact","target":{"kind":["bin"],"name":"some-other-tool"},"executable":"/repo/target/release/some-other-tool"}"#;
        let our_bin_line = format!(
            r#"{{"reason":"compiler-artifact","target":{{"kind":["bin"],"name":"{}"}},"executable":"/repo/custom-target/release/{}"}}"#,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_NAME")
        );
        assert_eq!(handle_cargo_json_line(lib_line, &mut log), None);
        assert_eq!(handle_cargo_json_line(other_bin_line, &mut log), None, "a dependency's own bin target must not be mistaken for ours");
        assert_eq!(handle_cargo_json_line(&our_bin_line, &mut log), Some(PathBuf::from(format!("/repo/custom-target/release/{}", env!("CARGO_PKG_NAME")))));
        assert!(logged.is_empty(), "artifact messages shouldn't themselves be logged verbatim: {logged:?}");
    }

    #[test]
    fn handle_cargo_json_line_logs_rendered_diagnostics_and_unparseable_lines() {
        let mut logged: Vec<String> = Vec::new();
        let diagnostic = r#"{"reason":"compiler-message","message":{"rendered":"warning: unused variable\n"}}"#;
        assert_eq!(handle_cargo_json_line(diagnostic, &mut |s| logged.push(s)), None);
        assert_eq!(logged, vec!["warning: unused variable".to_string()], "rendered diagnostics should be logged, trimmed, in plain text");

        logged.clear();
        assert_eq!(handle_cargo_json_line("not json at all", &mut |s| logged.push(s)), None);
        assert_eq!(logged, vec!["not json at all".to_string()], "an unparseable line must still reach the log, not be silently dropped");
    }

    #[test]
    fn restart_kind_execs_unless_launchd_will_relaunch_the_built_artifact() {
        let artifact = Path::new("/repo/target/release/agent-kanban");
        let elsewhere = Path::new("/repo/target/debug/agent-kanban");
        assert_eq!(restart_kind(None, Some(artifact), artifact), RestartKind::ExecSelf, "no supervisor: always exec");
        assert_eq!(restart_kind(Some("systemd"), Some(artifact), artifact), RestartKind::ExecSelf, "not launchd: always exec");
        assert_eq!(restart_kind(Some("launchd"), None, artifact), RestartKind::ExecSelf, "unknown running exe: can't confirm launchd would relaunch it");
        assert_eq!(
            restart_kind(Some("launchd"), Some(elsewhere), artifact),
            RestartKind::ExecSelf,
            "running from a different path than the artifact: launchd would relaunch the stale one"
        );
        assert_eq!(restart_kind(Some("launchd"), Some(artifact), artifact), RestartKind::ExitForSupervisor);
    }

    #[test]
    fn build_artifact_path_prefers_the_override() {
        let mut config = test_config();
        config.restart_artifact = Some(PathBuf::from("/somewhere/else/agent-kanban"));
        assert_eq!(
            build_artifact_path(&config, Some(Path::new("/repo/custom-target/release/agent-kanban"))),
            PathBuf::from("/somewhere/else/agent-kanban"),
            "an explicit restart_artifact wins even over a resolved cargo output path"
        );
    }

    #[test]
    fn build_artifact_path_falls_back_to_the_resolved_cargo_output() {
        let config = test_config();
        assert_eq!(
            build_artifact_path(&config, Some(Path::new("/repo/custom-target/release/agent-kanban"))),
            PathBuf::from("/repo/custom-target/release/agent-kanban"),
            "without an override, restart into wherever Cargo actually reported it built the binary"
        );
    }

    #[test]
    fn exec_self_command_runs_the_given_artifact() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact = tmp.path().join("new-server.sh");
        std::fs::write(&artifact, "#!/bin/sh\necho FRESH_BUILD\n").unwrap();
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = exec_self_command(&artifact).output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "FRESH_BUILD", "must hand off to the artifact it was given, not some other binary");
    }

    fn test_config() -> Config {
        Config {
            data_dir: PathBuf::from("/tmp"),
            bind: ([127, 0, 0, 1], 0).into(),
            public_url: "http://127.0.0.1:0".into(),
            container_url: "http://127.0.0.1:0".into(),
            no_auth: true,
            allow_host_agents: false,
            secrets: Default::default(),
            repo_dir: PathBuf::from("/repo"),
            restart_artifact: None,
            cargo_target_dir: None,
            running_exe: None,
            build_steps_override: None,
        }
    }
}
