use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

struct ContinuationProbe {
    name: &'static str,
    private: bool,
    read_only: bool,
    calls: Arc<AtomicU64>,
}

#[async_trait]
impl Tool for ContinuationProbe {
    fn name(&self) -> &'static str {
        self.name
    }

    fn title(&self) -> String {
        self.name.into()
    }

    fn description(&self) -> String {
        "Expose physical and task conversation identities for continuation tests.".into()
    }

    fn behavior(&self) -> crate::tool::ToolBehavior {
        crate::tool::ToolBehavior::new(
            self.read_only,
            !self.read_only,
            self.read_only,
            false,
            "Test-only continuation state probe.",
        )
    }

    fn meta(&self) -> Option<rmcp::model::MetaObject> {
        self.private
            .then(|| serde_json::from_value(json!({"ui":{"visibility":["app"]}})).unwrap())
    }

    fn input_schema(&self) -> Value {
        crate::tool::empty_object_schema()
    }

    fn output_schema(&self) -> Option<Value> {
        Some(json!({
            "type":"object",
            "properties":{
                "physical":{"type":"string"},
                "task":{"type":"string"}
            },
            "required":["physical", "task"],
            "additionalProperties":false
        }))
    }

    fn fills_structured_content(&self) -> bool {
        false
    }

    fn requires_project_root(&self) -> bool {
        false
    }

    async fn call(&self, _: Value, _: &AppConfig, _: &SessionState) -> ToolResult {
        ToolResult::error("Request context required")
    }

    async fn call_with_context(
        &self,
        _: Value,
        _: &AppConfig,
        _: &SessionState,
        context: &ToolRequestContext,
    ) -> ToolResult {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        let physical = context.conversation.as_ref().unwrap().stable_key();
        let task = context.task_conversation.as_ref().unwrap().stable_key();
        ToolResult::text(format!("{physical}:{task}"))
            .with_structured(json!({"physical":physical, "task":task}))
    }
}

fn continuation_request(name: &str, conversation: &str) -> CallToolRequestParams {
    serde_json::from_value(json!({
        "name":name,
        "arguments":{},
        "_meta":{"openai/session":conversation}
    }))
    .unwrap()
}

#[tokio::test]
async fn continuation_dispatch_uses_task_identity_and_retires_the_old_model_owner() {
    let root = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicU64::new(0));
    let mut handler = handler_with_tools(
        root.path(),
        vec![Box::new(ContinuationProbe {
            name: "continuation_model_probe",
            private: false,
            read_only: true,
            calls: calls.clone(),
        })],
        crate::types::ToolLogLevel::Info,
    );
    let continuations = Arc::new(
        crate::conversation_continuations::ConversationContinuationStore::new(
            root.path().join("continuations.json"),
        )
        .unwrap(),
    );
    let source = ConversationIdentity::from_openai_session("source-chat").unwrap();
    let destination = ConversationIdentity::from_openai_session("destination-chat").unwrap();
    let token = continuations
        .issue_token(&source, &source, root.path())
        .unwrap();
    continuations
        .claim(&destination, &token, |_, _| Ok(()))
        .unwrap();
    handler.continuations = continuations;

    let (server_transport, client_transport) = tokio::io::duplex(128 * 1024);
    let task = tokio::spawn(async move {
        handler
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_transport).await.unwrap();

    let result = client
        .call_tool(continuation_request(
            "continuation_model_probe",
            "destination-chat",
        ))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    let output = result.structured_content.unwrap();
    assert_eq!(output["physical"], destination.stable_key());
    assert_eq!(output["task"], source.stable_key());

    let retired = client
        .call_tool(continuation_request(
            "continuation_model_probe",
            "source-chat",
        ))
        .await
        .unwrap();
    assert_eq!(retired.is_error, Some(true));
    assert!(
        retired
            .content
            .iter()
            .any(|block| block.as_text().is_some_and(|text| {
                text.text
                    .contains(crate::conversation_continuations::RETIRED_MESSAGE)
            }))
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

    client.cancel().await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn retired_conversation_keeps_private_reads_but_cannot_use_private_mutations() {
    let root = tempfile::tempdir().unwrap();
    let read_calls = Arc::new(AtomicU64::new(0));
    let write_calls = Arc::new(AtomicU64::new(0));
    let mut handler = handler_with_tools(
        root.path(),
        vec![
            Box::new(ContinuationProbe {
                name: "continuation_private_read",
                private: true,
                read_only: true,
                calls: read_calls.clone(),
            }),
            Box::new(ContinuationProbe {
                name: "continuation_private_write",
                private: true,
                read_only: false,
                calls: write_calls.clone(),
            }),
        ],
        crate::types::ToolLogLevel::Info,
    );
    let continuations = Arc::new(
        crate::conversation_continuations::ConversationContinuationStore::new(
            root.path().join("continuations.json"),
        )
        .unwrap(),
    );
    let source = ConversationIdentity::from_openai_session("source-chat").unwrap();
    let destination = ConversationIdentity::from_openai_session("destination-chat").unwrap();
    let token = continuations
        .issue_token(&source, &source, root.path())
        .unwrap();
    continuations
        .claim(&destination, &token, |_, _| Ok(()))
        .unwrap();
    handler.continuations = continuations;

    let (server_transport, client_transport) = tokio::io::duplex(128 * 1024);
    let task = tokio::spawn(async move {
        handler
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_transport).await.unwrap();

    let read = client
        .call_tool(continuation_request(
            "continuation_private_read",
            "source-chat",
        ))
        .await
        .unwrap();
    assert_ne!(read.is_error, Some(true));
    assert_eq!(
        read.structured_content.unwrap()["task"],
        source.stable_key()
    );

    let write = client
        .call_tool(continuation_request(
            "continuation_private_write",
            "source-chat",
        ))
        .await
        .unwrap();
    assert_eq!(write.is_error, Some(true));
    assert_eq!(read_calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(write_calls.load(AtomicOrdering::SeqCst), 0);

    client.cancel().await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn retired_rejection_happens_before_agent_ticket_reservation() {
    let root = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicU64::new(0));
    let mut handler = handler_with_tools(
        root.path(),
        vec![Box::new(ContinuationProbe {
            name: "retired_ticket_probe",
            private: false,
            read_only: true,
            calls: calls.clone(),
        })],
        crate::types::ToolLogLevel::Info,
    );
    let mut config = handler.config.as_ref().clone();
    config.experimental.agent_tickets = true;
    handler.config = Arc::new(config);
    let ticket_dir = root.path().join("tickets");
    handler.agent_tickets = Arc::new(AgentTicketStore::persistent(ticket_dir.clone()));
    let continuations = Arc::new(
        crate::conversation_continuations::ConversationContinuationStore::new(
            root.path().join("continuations.json"),
        )
        .unwrap(),
    );
    let source = ConversationIdentity::from_openai_session("ticket-source").unwrap();
    let destination = ConversationIdentity::from_openai_session("ticket-destination").unwrap();
    let token = continuations
        .issue_token(&source, &source, root.path())
        .unwrap();
    continuations
        .claim(&destination, &token, |_, _| Ok(()))
        .unwrap();
    handler.continuations = continuations;

    let (server_transport, client_transport) = tokio::io::duplex(128 * 1024);
    let task = tokio::spawn(async move {
        handler
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_transport).await.unwrap();
    let retired = client
        .call_tool(continuation_request(
            "retired_ticket_probe",
            "ticket-source",
        ))
        .await
        .unwrap();
    assert_eq!(retired.is_error, Some(true));
    assert!(
        retired
            .content
            .iter()
            .any(|block| block.as_text().is_some_and(|text| {
                text.text
                    .contains(crate::conversation_continuations::RETIRED_MESSAGE)
            }))
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);

    client.cancel().await.unwrap();
    task.await.unwrap();
    assert!(!ticket_dir.exists() || std::fs::read_dir(&ticket_dir).unwrap().next().is_none());
    let audit = std::fs::read_to_string(root.path().join("audit.jsonl")).unwrap();
    assert!(!audit.contains("\"event\":\"ticket_reservation\""));
}

#[tokio::test]
async fn prepare_and_continue_tools_transfer_the_same_task_without_copying_state() {
    use crate::tools::continuation::{CONTINUATION_META, ContinueTask, PrepareContinuation};

    let root = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicU64::new(0));
    let mut handler = handler_with_tools(
        root.path(),
        vec![
            Box::new(PrepareContinuation),
            Box::new(ContinueTask),
            Box::new(ContinuationProbe {
                name: "continued_task_probe",
                private: false,
                read_only: true,
                calls: calls.clone(),
            }),
        ],
        crate::types::ToolLogLevel::Info,
    );
    let continuations = Arc::new(
        crate::conversation_continuations::ConversationContinuationStore::new(
            root.path().join("continuations.json"),
        )
        .unwrap(),
    );
    handler.continuations = continuations.clone();

    let (server_transport, client_transport) = tokio::io::duplex(128 * 1024);
    let task = tokio::spawn(async move {
        handler
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_transport).await.unwrap();

    let prepared = client
        .call_tool(continuation_request(
            PrepareContinuation::NAME,
            "source-chat",
        ))
        .await
        .unwrap();
    assert_ne!(prepared.is_error, Some(true));
    let token = prepared.meta.as_ref().unwrap()[CONTINUATION_META]["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!prepared.content.iter().any(|block| {
        block
            .as_text()
            .is_some_and(|text| text.text.contains(&token))
    }));

    let claimed = client
        .call_tool(
            serde_json::from_value(json!({
                "name":ContinueTask::NAME,
                "arguments":{"continuationToken":token.clone()},
                "_meta":{"openai/session":"destination-chat"}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(claimed.is_error, Some(true));
    assert_eq!(
        claimed.structured_content.as_ref().unwrap()["continued"],
        true
    );

    let repeated = client
        .call_tool(
            serde_json::from_value(json!({
                "name":ContinueTask::NAME,
                "arguments":{"continuationToken":token},
                "_meta":{"openai/session":"destination-chat"}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(repeated.is_error, Some(true));
    assert_eq!(
        repeated.structured_content.as_ref().unwrap()["continued"],
        true
    );

    let other_source = ConversationIdentity::from_openai_session("other-source-chat").unwrap();
    let other_token = continuations
        .issue_token(&other_source, &other_source, root.path())
        .unwrap();
    let wrong_task = client
        .call_tool(
            serde_json::from_value(json!({
                "name":ContinueTask::NAME,
                "arguments":{"continuationToken":other_token},
                "_meta":{"openai/session":"destination-chat"}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_task.is_error, Some(true));
    assert!(matches!(
        continuations.resolve(&other_source).unwrap(),
        crate::conversation_continuations::ConversationOwnership::Active { .. }
    ));

    let probe = client
        .call_tool(continuation_request(
            "continued_task_probe",
            "destination-chat",
        ))
        .await
        .unwrap();
    let source = ConversationIdentity::from_openai_session("source-chat").unwrap();
    assert_eq!(
        probe.structured_content.unwrap()["task"],
        source.stable_key()
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

    let retired = client
        .call_tool(continuation_request(
            PrepareContinuation::NAME,
            "source-chat",
        ))
        .await
        .unwrap();
    assert_eq!(retired.is_error, Some(true));

    client.cancel().await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn handoff_keeps_workspace_chat_memory_diff_and_exec_state_attached() {
    use crate::conversation_continuations::{ConversationContinuationStore, ConversationOwnership};
    use crate::exec_sessions::ConversationExecSessionStore;
    use crate::markdown_chat::MarkdownChatStore;
    use crate::memory::{create_note, load_memory, memory_dir, save_plan};
    use crate::types::{PlanItem, PlanState, PlanStepStatus, WorktreeMode};

    let root = tempfile::tempdir().unwrap();
    let access_root = root.path().join("projects");
    let project = access_root.join("demo");
    std::fs::create_dir_all(&project).unwrap();
    let mut config = crate::config::default_config(access_root);
    config.multi_project = true;
    config.worktrees.mode = WorktreeMode::Never;
    config.markdown_chat.enabled = true;
    config.memory.dir = Some(root.path().join("metadata").to_string_lossy().into_owned());

    let source = ConversationIdentity::from_openai_session("source-state").unwrap();
    let destination = ConversationIdentity::from_openai_session("destination-state").unwrap();
    let bindings = ProjectBindingStore::new(root.path().join("bindings"));
    let selection = bindings
        .select_project_root(&config, &source, "demo")
        .await
        .unwrap();
    let effective = bindings.effective_config(&config, &source).unwrap();
    assert_eq!(
        selection.project_root,
        std::fs::canonicalize(&project).unwrap()
    );

    let session = SessionState::new();
    let chats = MarkdownChatStore::default();
    let source_chat = chats.chat(&effective, Some(&source), &session).unwrap();
    let sent = source_chat
        .append_user(
            "handoff-state".into(),
            "Keep this agent-chat history.".into(),
        )
        .await
        .unwrap();
    assert!(
        source_chat
            .read(true)
            .await
            .unwrap()
            .text
            .contains("Keep this")
    );
    source_chat.mark_delivered(sent.end).await.unwrap();

    assert!(
        create_note(
            &effective,
            "continuation-handoff",
            "Resume the same task",
            "2026-10-01T00:00:00Z"
        )
        .ok
    );
    assert!(save_plan(
        &effective,
        Some(PlanState {
            explanation: Some("Preserved plan".into()),
            plan: vec![PlanItem {
                step: "Finish the handoff".into(),
                status: PlanStepStatus::InProgress,
            }],
        })
    ));

    let exec_sessions = ConversationExecSessionStore::new();
    let source_exec = exec_sessions.session_for(&source, &session);
    let source_diff = DiffOwner::conversation(&source);

    let continuations =
        ConversationContinuationStore::new(root.path().join("continuations.json")).unwrap();
    let token = continuations
        .issue_token(&source, &source, &selection.project_root)
        .unwrap();
    continuations
        .claim(&destination, &token, |_, workspace| {
            assert_eq!(workspace, selection.project_root);
            Ok(())
        })
        .unwrap();
    let ConversationOwnership::Active { task } = continuations.resolve(&destination).unwrap()
    else {
        panic!("destination must own the continued task");
    };

    assert_eq!(
        bindings.selected_project_root(&config, &task).unwrap(),
        Some(selection.project_root.clone())
    );
    let continued = bindings.effective_config(&config, &task).unwrap();
    assert_eq!(continued.work_dir, effective.work_dir);
    assert_eq!(memory_dir(&continued), memory_dir(&effective));
    let memory = load_memory(&continued);
    assert_eq!(
        memory.notes["continuation-handoff"].value,
        "Resume the same task"
    );
    assert_eq!(memory.plan.unwrap().plan[0].step, "Finish the handoff");

    let destination_chat = chats.chat(&continued, Some(&task), &session).unwrap();
    assert_eq!(destination_chat.path(), source_chat.path());
    assert!(destination_chat.read(false).await.unwrap().text.is_empty());
    let page = destination_chat.widget_page(None, None).await.unwrap();
    assert_eq!(page.read_through, sent.end);
    assert!(
        page.messages
            .iter()
            .any(|message| { message.markdown == "Keep this agent-chat history." })
    );

    let destination_exec = exec_sessions.session_for(&task, &session);
    assert!(source_exec.shares_exec_state(&destination_exec));
    assert_eq!(
        source_diff.test_conversation_key(),
        DiffOwner::conversation(&task).test_conversation_key()
    );
    let chat_count = walkdir::WalkDir::new(memory_dir(&continued).join("chats"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() == "CHAT.md")
        .count();
    assert_eq!(chat_count, 1);
}
