# Conversation Handoff Transfer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace workspace-only continuation prompts with a one-time handoff that moves one Codexify task to a replacement ChatGPT conversation while preserving Codexify-owned state and retiring the old owner.

**Architecture:** Add a private persistent continuation store that maps physical ChatGPT conversations to a stable task identity and issues one-time token capabilities. Resolve project/chat/diff/exec state through the task identity while retaining physical identity for authorization, connector schemas, audit, and agent tickets. The setup widget creates the token only when the user copies the prompt; the destination claims it through `continue_task` before workspace selection.

**Tech Stack:** Rust 2024, Tokio, serde/serde_json, SHA-256, getrandom, base64 URL-safe tokens, MCP Apps HTML/JavaScript, Node and Playwright tests.

---

### Task 1: Persist continuation ownership and tokens

**Files:**
- Create: `src/conversation_continuations.rs`
- Modify: `src/lib.rs`
- Test: `src/conversation_continuations.rs`

- [ ] **Step 1: Write failing store tests**

Add unit tests covering:

```rust
#[test]
fn issued_tokens_are_random_and_only_digests_are_persisted() { /* ... */ }

#[test]
fn reload_resolves_the_current_owner_to_the_original_task_identity() { /* ... */ }

#[test]
fn a_new_token_revokes_the_previous_token_for_the_same_generation() { /* ... */ }

#[test]
fn one_claim_wins_and_replay_is_rejected() { /* ... */ }

#[test]
fn failed_claim_validation_preserves_owner_and_token() { /* ... */ }

#[test]
fn claim_rejects_while_the_source_has_a_model_call_in_flight() { /* ... */ }
```

Use `ConversationIdentity::from_openai_session` fixtures, a temporary state directory, and a validator closure that checks the expected workspace.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```sh
cargo test conversation_continuations --lib
```

Expected: compilation failure because `conversation_continuations` does not exist.

- [ ] **Step 3: Implement the store**

Create:

```rust
pub struct ConversationContinuationStore { /* private state, in-flight counts */ }

pub enum ConversationOwnership {
    Active { task: ConversationIdentity },
    Retired { task: ConversationIdentity },
}

pub struct ModelCallGuard { /* decrements source count on Drop */ }

pub struct ClaimedContinuation {
    pub task: ConversationIdentity,
    pub workspace: PathBuf,
}
```

Implement:

```rust
pub fn for_current_user() -> Result<Self, String>;
pub fn new(path: PathBuf) -> Result<Self, String>;
pub fn resolve(&self, physical: &ConversationIdentity) -> Result<ConversationOwnership, String>;
pub fn begin_model_call(self: &Arc<Self>, physical: &ConversationIdentity) -> Result<(ConversationIdentity, ModelCallGuard), String>;
pub fn issue_token(&self, physical: &ConversationIdentity, task: &ConversationIdentity, workspace: &Path) -> Result<String, String>;
pub fn claim<F>(&self, physical: &ConversationIdentity, token: &str, validate: F) -> Result<ClaimedContinuation, String>
where F: FnOnce(&ConversationIdentity, &Path) -> Result<(), String>;
```

Persist one bounded versioned JSON document with private permissions and atomic replacement. Store token digests, never raw tokens. Validate every task/owner key as lowercase 64-character hexadecimal, every token digest as hexadecimal, and every workspace as absolute.

- [ ] **Step 4: Run the focused tests and verify they pass**

Run:

```sh
cargo test conversation_continuations --lib
```

Expected: all continuation-store tests pass.

- [ ] **Step 5: Commit**

```sh
git add src/conversation_continuations.rs src/lib.rs
git commit -m "feat: persist conversation handoff ownership"
```

### Task 2: Separate physical and task conversation identities at dispatch

**Files:**
- Modify: `src/tool.rs`
- Modify: `src/server.rs`
- Modify: `src/tools/setup.rs`
- Modify: `src/tools/markdown_chat.rs`
- Modify: `src/tools/markdown_chat_ui.rs`
- Modify: `src/tools/get_agent_brief.rs`
- Modify: `src/tools/show_diff.rs`
- Modify: `src/tools/workspace_ui.rs`
- Modify: `src/markdown_chat/mod.rs`
- Test: `src/server_continuation_tests.rs`
- Modify: `src/server.rs` test includes and handler fixtures

- [ ] **Step 1: Write failing dispatch tests**

Add server tests proving:

```rust
#[tokio::test]
async fn task_state_uses_the_original_identity_after_handoff() { /* project, chat, diff, exec */ }

#[tokio::test]
async fn retired_model_calls_fail_before_ticket_reservation_and_dispatch() { /* ... */ }

#[tokio::test]
async fn retired_app_reads_work_but_app_mutations_are_rejected() { /* ... */ }
```

Record actual tool-call counters and audit/ticket state so the test demonstrates that a retired call never reaches either subsystem.

- [ ] **Step 2: Run tests and verify the expected failure**

Run:

```sh
cargo test --lib continuation
```

Expected: failures because dispatch does not resolve task ownership.

- [ ] **Step 3: Extend request context**

Change `ToolRequestContext` to carry both:

```rust
pub conversation: Option<ConversationIdentity>,
pub task_conversation: Option<ConversationIdentity>,
pub conversation_retired: bool,
```

Keep `conversation` as the physical request identity. Update stateful tools to use `task_conversation`; keep setup authorization and connector-schema handling on `conversation`.

- [ ] **Step 4: Resolve ownership before dispatch**

In `CodexHandler::call_tool`:

- derive the physical conversation;
- resolve the task identity;
- acquire a `ModelCallGuard` for model-visible calls;
- reject retired model calls immediately;
- permit retired app-only tools only when `tool.behavior().read_only` is true;
- keep agent tickets, authorization, schema tracking, and audit attribution physical;
- use task identity for project binding, chat activity/delivery, exec sessions, diff ownership, workspace-change state, and selected-root lookup.

Re-resolve task ownership after `continue_task` so the successful claim call finishes against the transferred task state.

- [ ] **Step 5: Run dispatch tests**

Run:

```sh
cargo test --lib continuation
cargo test --test markdown_chat_tools --test meta_suite
```

Expected: all selected tests pass.

- [ ] **Step 6: Commit**

```sh
git add src/tool.rs src/server.rs src/server_continuation_tests.rs src/tools src/markdown_chat/mod.rs
git commit -m "feat: route task state through handoff ownership"
```

### Task 3: Create and claim continuation tokens through tools

**Files:**
- Create: `src/tools/continuation.rs`
- Modify: `src/tools/mod.rs`
- Modify: `src/registry.rs`
- Modify: `src/tools/set_project_root.rs`
- Modify: `src/server.rs`
- Test: `tests/workspace_resume.rs`
- Test: `src/server_continuation_tests.rs`

- [ ] **Step 1: Write failing tool-contract tests**

Cover:

```rust
#[test]
fn continue_task_accepts_only_a_write_only_continuation_token() { /* schema and parser */ }

#[tokio::test]
async fn continuation_claim_reuses_the_exact_existing_workspace() { /* ... */ }

#[tokio::test]
async fn continuation_claim_preserves_chat_cursor_memory_diff_and_exec_state() { /* ... */ }

#[tokio::test]
async fn invalid_claim_never_falls_back_to_resume_path_or_new_selection() { /* ... */ }
```

- [ ] **Step 2: Verify tests fail**

Run:

```sh
cargo test continuation_claim
cargo test continue_task_accepts_only_a_write_only_continuation_token
```

Expected: failures because the tool argument and private token tool do not exist.

- [ ] **Step 3: Add the private token tool**

Create `setup_ui_prepare_continuation` as an app-only non-read-only tool. It requires a selected project root, verifies the current physical conversation is the active owner, calls `issue_token`, and returns the token only in component metadata:

```json
{
  "io.github.devnoname120/codexify/continuation": {
    "token": "...",
    "workspace": "/absolute/path"
  }
}
```

Its model-visible text is only `Continuation prompt ready.`

- [ ] **Step 4: Add `continue_task`**

Add a dedicated model-facing `continue_task({continuationToken})` tool. Keep
`resumePath` on `set_project_root` for workspace-only compatibility. On claim,
validate that the destination has no selected workspace or other task, verify the
token's source workspace through the source task binding, commit the owner change,
and return the existing workspace with continuation-specific text. Repeating the
exact successful token from the new owner is idempotent; a different token is not.

Update `RESUME_GUIDANCE` and schema descriptions to distinguish `continue_task`
full task continuation from `resumePath`.

- [ ] **Step 5: Run focused tests**

Run:

```sh
cargo test continuation_claim
cargo test --test workspace_resume
cargo test --test meta_suite
```

Expected: all focused tests pass.

- [ ] **Step 6: Commit**

```sh
git add src/tools/continuation.rs src/tools/mod.rs src/registry.rs src/tools/set_project_root.rs src/server.rs src/server_continuation_tests.rs tests/workspace_resume.rs
git commit -m "feat: claim task handoffs with one-time tokens"
```

### Task 4: Update embedded and standalone chat UI behavior

**Files:**
- Modify: `src/setup_ui.html`
- Modify: `src/markdown_chat_ui.html`
- Modify: `src/tools/setup.rs`
- Modify: `src/tools/markdown_chat_ui.rs`
- Test: `scripts/test-setup-widget.mjs`
- Test: `scripts/test-markdown-chat-widget.mjs`
- Test: `scripts/test-owner-chat.mjs`

- [ ] **Step 1: Write failing widget tests**

Add tests that assert:

- copying a continuation prompt calls `setup_ui_prepare_continuation` and embeds only `continuationToken`;
- repeated copy requests use the latest token;
- the prompt no longer says chat history or command sessions are lost;
- setup state exposes retirement and disables old actions;
- a retired chat widget renders existing history, shows a read-only notice, and disables its composer;
- the standalone owner view still lists one chat because both physical conversations resolve the same task chat directory.

- [ ] **Step 2: Run widget tests and verify failure**

Run:

```sh
node --test scripts/test-setup-widget.mjs
node --test scripts/test-markdown-chat-widget.mjs
node --test scripts/test-owner-chat.mjs
```

Expected: failures on missing token call and retirement state.

- [ ] **Step 3: Generate the prompt at copy time**

Keep the visible continuation textarea but generate/refresh its token when the user clicks **Copy continuation prompt**. Parse the component-only token metadata, rebuild the prompt, copy it, and retain manual-selection fallback if clipboard access fails.

Keep **Prepare handoff** focused on updating task plan/memory. Replace the visible help and prompt text with plain language describing preserved state and old-chat read-only behavior.

- [ ] **Step 4: Add retired widget state**

Add a bounded continuation-status object to setup and chat widget payloads. Disable setup actions and the embedded chat composer for retired physical conversations while leaving history polling available. Keep standalone owner chat writable because its API addresses the stable task chat.

- [ ] **Step 5: Run widget tests**

Run:

```sh
node --test scripts/test-setup-widget.mjs
node --test scripts/test-markdown-chat-widget.mjs
node --test scripts/test-owner-chat.mjs
```

Expected: all widget tests pass in their configured engines.

- [ ] **Step 6: Commit**

```sh
git add src/setup_ui.html src/markdown_chat_ui.html src/tools/setup.rs src/tools/markdown_chat_ui.rs scripts/test-setup-widget.mjs scripts/test-markdown-chat-widget.mjs scripts/test-owner-chat.mjs
git commit -m "feat: continue one task across ChatGPT conversations"
```

### Task 5: Documentation, migration, and complete verification

**Files:**
- Modify: `src/instructions.rs`
- Modify: `docs/REFERENCE.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `CHANGELOG.md`
- Modify: relevant test fixtures containing old continuation copy

- [ ] **Step 1: Update instructions and docs**

Document:

- full task continuation through `continuationToken`;
- workspace-only compatibility through `resumePath`;
- state that stays attached and state that remains physical-conversation scoped;
- old conversation read-only behavior;
- token persistence, privacy, replay handling, and restart behavior;
- native ChatGPT history limitation.

Remove claims that agent chat, diff state, or running command sessions are necessarily lost in the setup-card handoff.

- [ ] **Step 2: Format and lint**

Run:

```sh
cargo fmt --all
rustfmt --edition 2024 --check src/server_markdown_chat_tests.rs src/server_markdown_chat_widget_tests.rs src/server_workspace_tests.rs src/server_agent_ticket_tests.rs src/server_continuation_tests.rs
cargo clippy --all-targets -- -D warnings
```

Expected: exit 0 with no warnings.

- [ ] **Step 3: Run Rust tests**

Run:

```sh
cargo test --all
```

Expected: all non-environment-dependent tests pass.

- [ ] **Step 4: Run browser tests**

Run:

```sh
node --test scripts/test-setup-widget.mjs
node --test scripts/test-markdown-chat-widget.mjs
node --test --test-concurrency=1 scripts/test-workspace-widget.mjs scripts/test-workspace-picker.mjs scripts/test-owner-chat.mjs scripts/test-chat-workspace-switch.mjs
```

Expected: all Chromium/WebKit tests pass.

- [ ] **Step 5: Review the final diff and repository state**

Run `show_diff`, inspect all changed files, verify no token or local path fixture leaked, and confirm `git status --short` contains only intended files.

- [ ] **Step 6: Commit and push**

```sh
git add CHANGELOG.md docs src tests scripts
git commit -m "feat: transfer task state between conversations"
git push origin HEAD:main
gh api repos/devnoname120/codexify/commits/main --jq '{sha:.sha,subject:.commit.message}'
```

Expected: push succeeds without force and GitHub reports the new commit at `main`.
