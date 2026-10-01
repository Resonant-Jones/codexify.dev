# Conversation Handoff Transfer Design

## Goal

Let a user replace a stale or otherwise unusable ChatGPT conversation without starting a separate Codexify task. The replacement conversation keeps the same workspace, agent-chat transcript, saved plan and notes, diff history, and resident command sessions. Once the replacement claims the handoff, the old ChatGPT conversation becomes read-only.

## User experience

The setup card keeps the existing **Prepare handoff** and **Copy continuation prompt** flow.

**Prepare handoff** asks the current agent to update the saved plan and the `continuation-handoff` memory note.

When the user copies the continuation prompt, the setup widget asks Codexify for a one-time continuation token and embeds it in a prompt containing:

```json
{"continuationToken":"<opaque token>"}
```

The user pastes that prompt into a new ChatGPT conversation using the same connector. After normal setup and authorization, the new conversation calls `continue_task` with only `continuationToken` before selecting a workspace.

A successful claim keeps one Codexify task and changes which ChatGPT conversation owns it. No workspace, chat, plan, or cursor is copied.

The setup card copy becomes:

```text
Prepare a handoff, then continue this Codexify task in a new ChatGPT conversation. The workspace, agent-chat history, saved plan, memory, command sessions, and task state stay attached. Once the new conversation claims the handoff, this conversation becomes read-only.
```

## Conversation identity

Codexify continues to derive a physical conversation identity from `openai/session`. A new continuation store maps physical ChatGPT conversations to one stable Codexify task identity.

For an ordinary conversation with no handoff record, the physical and task identities are the same. The first continuation token lazily creates a task record using the current conversation identity as the stable task identity. This avoids migrating existing project bindings, chat files, diff refs, or command-session maps.

A task record contains:

- the stable task identity;
- its legacy identity hash for existing migration paths;
- the current physical ChatGPT owner;
- a generation incremented after each completed handoff.

Alias records map every physical conversation that has owned the task to that task identity. Only the current owner may make model-facing Codexify calls.

## Persistent continuation state

Continuation state lives outside project repositories under the user's Codexify state directory. It contains only hashed conversation identifiers, absolute workspace paths already known to Codexify, generations, and SHA-256 digests of random continuation tokens. Raw `openai/session` values and raw tokens are never persisted.

The file and its parent directory use private permissions and reject symlinks or malformed state. Updates use a temporary file and rename so restart cannot expose a partial document.

A continuation token is:

- generated from 32 random bytes and URL-safe base64;
- stored only by digest;
- valid for one task generation;
- revoked when a newer token is issued for the same task;
- consumed by exactly one successful claim;
- retained after a failed claim so the user can retry.

After a completed claim, the task record retains the digest and workspace of that
claim. This lets the new owner safely repeat the exact `continue_task` call after
a lost response or service restart. A different token cannot be treated as a
retry or attach an already-owned conversation to another task.

## Call routing

Each tool call has two identities:

- **request conversation**: the physical ChatGPT conversation, used for setup authorization, connector-schema tracking, audit attribution, and the duplicate-agent ticket chain;
- **task conversation**: the stable Codexify task identity, used for project binding, agent chat, diff state, resident command sessions, workspace-change state, and task activity.

The duplicate-agent ticket chain remains physical-conversation scoped. The continuation claim starts the replacement conversation's own ticket chain. The old conversation is rejected by the handoff ownership check before ticket validation, so it cannot regain control after ticket expiry.

## Ownership and concurrency

Codexify tracks the number of active model-facing calls for each physical conversation in memory.

A continuation claim succeeds only when:

- the token is valid and belongs to the current task generation;
- the caller is a different, normally authorized ChatGPT conversation;
- the caller has not selected another workspace or joined another task;
- the saved task workspace still matches the token record;
- the old owner has no model-facing call in flight.

The claim updates the owner, records the destination alias, increments the generation, removes outstanding tokens for the task, and persists the state before returning success.

If validation or persistence fails, the old owner remains active and the token remains usable. Concurrent claims have one winner.

Resident shell processes are not considered in-flight tool calls. Their session map is keyed by the task identity, so the replacement can continue using known session IDs while the server remains running. An individual tool call already executing in the old conversation prevents the transfer until that call returns. As before, a Codexify service restart terminates resident processes.

## Retired conversations

After a successful claim, model-facing calls from an old owner fail before ticket reservation or tool dispatch with:

```text
This Codexify task continued in another ChatGPT conversation. This conversation is now read-only.
```

App-only read operations may still render the old setup and chat history. App-only mutations, including sending a chat message or changing workspace selection, are rejected.

Setup and chat widget state include a retired flag. A retired setup card disables handoff, update, project-selection, and workspace-switch actions. A retired chat widget disables its composer and shows a short read-only notice. The standalone owner chat is keyed by the stable task identity and therefore continues to show one conversation and one transcript.

## State preserved

Because all task-scoped operations resolve the stable task identity, the replacement keeps:

- the exact project or scratch binding;
- tracked, staged, unstaged, and untracked files;
- the existing agent-chat `CHAT.md`, cursor, delivery receipts, and activity record;
- saved plan and memory notes;
- the `continuation-handoff` note;
- incremental and project-open diff checkpoints;
- resident command sessions;
- the same standalone owner-chat entry.

Historical user messages stay acknowledged according to the existing shared cursor and are not replayed as new input.

## State not preserved

The replacement conversation must complete normal connector setup and authorization before claiming the handoff. Connector-schema state, audit attribution, and the duplicate-agent ticket chain remain tied to the physical ChatGPT conversation.

Codexify cannot copy the native ChatGPT webpage transcript because ChatGPT does not expose it through the connector. The handoff note remains necessary for relevant reasoning that occurred only in ordinary ChatGPT messages and was never recorded in agent chat, files, plan, or memory.

## Compatibility

Existing conversations without continuation records behave exactly as before.

The old `resumePath` continuation remains accepted for manually resuming only a workspace, but its documentation clearly distinguishes it from full task continuation. New setup-card prompts call `continue_task` with `continuationToken`.

Existing chat paths, project bindings, diff refs, command sessions, and scratch workspaces do not move. The task identity initially equals the original conversation's existing stable key, so state becomes shared by resolving aliases rather than by copying or renaming files.

## Error handling

Errors are explicit and do not fall back to another checkout or create a new task:

- invalid, expired, revoked, or consumed token;
- continuation attempted from the same physical conversation;
- destination already bound to a workspace or another task;
- source task no longer owns the token generation;
- source workspace missing or changed;
- source model call still in flight;
- malformed or unsafe continuation state on disk;
- failure to persist the ownership change.

No error includes raw conversation identifiers or token values.

## Verification

Tests cover:

- token generation, private persistence, digest-only storage, restart reload, and revocation;
- one winner for concurrent claims and rejection of replay;
- failure while the source has an in-flight model call;
- unchanged ownership after every failed claim path;
- request identity versus task identity routing;
- preserved project binding, chat path and cursor, messages, memory, plan, diff refs, and resident command sessions;
- no replay of already acknowledged chat messages;
- old model-call rejection before agent-ticket reservation;
- read-only old widget state and blocked app-only mutations;
- unchanged behavior for legacy conversations;
- setup-widget prompt generation and user-facing copy;
- one standalone-chat entry across repeated handoffs;
- restart behavior after one and multiple handoffs.
