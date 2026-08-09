# TODO — crow (zed fork)

Fork-local tracking list. Upstream is `zed-industries/zed` (`upstream` remote);
our work is on `origin` (`odellus/zed`). This file is for *our* bugs and tasks,
not upstream's.

The thesis we're shaking out: **the ACP client is a first-class editor panel
citizen.** Most of the rough edges live at that seam.

---

## Bugs

### ACP panel message routing — input delivered to the wrong session
- **Date:** 2026-07-31
- **Observed:** A user message typed for session A (`stirring-realistic-antelope`)
  was delivered to a different live session B (`chivalrous-fragrant-firefly`).
  No error, no warning — session B's agent just answered a message it was never
  meant to receive, and session A saw nothing. Silent misrouting.
- **Expected:** Input entered in a panel targets *that panel's* session. When
  multiple ACP sessions are live, "where does the next message go" must be
  well-defined (foreground/focused panel should win) and a misroute should fail
  loudly, not silently.
- **Suspected area:** session targeting / input dispatch in the ACP panel layer
  (`crates/crow-acp/`, panel ↔ `AcpSession` wiring in `src-tauri/`). Likely no
  explicit focus/foreground signal is being used to pick the target session.
- **Open questions:**
  - What *is* the current routing rule (last-active? last-created? arbitrary)?
  - Is there a foreground/focus signal available from the workbench to key off?
  - Should a session be able to receive a message "addressed" to another, or
    should that be a hard error?
- **Related class of corner cases:** ambiguous continuation across N live
  sessions; agent identity under delegation ("what did *you* say" when a session
  has agents a1–a3).

### ~~Client-side `read`/`write` tools scoped to the project-root cwd~~ — FIXED 2026-07-31
- **Date found:** 2026-07-31
- **Symptom:** The agent's `read`/`write` tools (client-side, ACP
  `fs/read_text_file` / `fs/write_text_file`) only operated inside a project
  worktree. Paths outside the worktree roots (e.g. `~/.crow/notes/`,
  `~/.crow/skills/`) failed — read with "Resource not found", write with a
  generic "Internal error". Root cause: `acp_thread.rs` gated both on
  `project_path_for_absolute_path(...)`, which returns `None` for any path not
  contained in a worktree, and otherwise routed everything through the editor
  Buffer model.
- **Fix:** Added a fallback in `crates/acp_thread/src/acp_thread.rs`
  `read_text_file` / `write_text_file`: when the path is outside every worktree,
  do direct I/O via the project's `Fs` service (`fs.load` / `fs.write` /
  `fs.create_dir`) instead of erroring. In-worktree paths still go through the
  editor Buffer model (transactions, format-on-save, action-log) unchanged.
  Using `Fs` (not raw `smol::fs`) keeps it on the deterministic test scheduler,
  so the `FakeFs`-backed unit tests still pass. Verified end-to-end in the
  running editor.
- **Merge-gate implication:** this is a Category-B crow modification to a shared,
  frequently-edited upstream file. Upstream merges touching
  `read_text_file`/`write_text_file` must PRESERVE the out-of-worktree fallback.
  Added to the protected manifest. Characterization-test candidate: reading a
  path outside every worktree succeeds (no `ResourceNotFound`).

---

## Tasks

- [ ] **Upstream watch** — near-daily digest of new `zed-industries/zed`
      activity (new commits on `upstream/main`, new tags). Implemented as
      `script/upstream-watch` + cron; digest lands in
      `~/.crow/notes/dev/upstream-zed-digest.md`.
