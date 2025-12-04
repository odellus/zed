//! Trace types for crow telemetry

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Role of the agent making an LLM call
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRole {
    Executor,
    Discriminator,
    EditAgent,
    DiffJudge,
    /// External agent: Claude Code via ACP
    ExternalClaudeCode,
    /// External agent: Gemini via ACP
    ExternalGemini,
    /// External agent: Custom/other via ACP
    ExternalCustom,
}

impl AgentRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentRole::Executor => "executor",
            AgentRole::Discriminator => "discriminator",
            AgentRole::EditAgent => "edit_agent",
            AgentRole::DiffJudge => "diff_judge",
            AgentRole::ExternalClaudeCode => "external_claude_code",
            AgentRole::ExternalGemini => "external_gemini",
            AgentRole::ExternalCustom => "external_custom",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "executor" => Some(AgentRole::Executor),
            "discriminator" => Some(AgentRole::Discriminator),
            "edit_agent" => Some(AgentRole::EditAgent),
            "diff_judge" => Some(AgentRole::DiffJudge),
            "external_claude_code" => Some(AgentRole::ExternalClaudeCode),
            "external_gemini" => Some(AgentRole::ExternalGemini),
            "external_custom" => Some(AgentRole::ExternalCustom),
            _ => None,
        }
    }

    /// Returns true if this is an external agent (ACP-based)
    pub fn is_external(&self) -> bool {
        matches!(
            self,
            AgentRole::ExternalClaudeCode | AgentRole::ExternalGemini | AgentRole::ExternalCustom
        )
    }
}

/// Full telemetry record for an LLM call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub id: String,
    pub session_id: String,
    pub thread_id: Option<String>,
    pub prompt_id: Option<String>,
    pub agent_role: AgentRole,
    pub model_provider: String,
    pub model_id: String,
    pub template_inputs: Option<String>,
    pub rendered_prompt: Option<String>,
    pub request_messages: String,
    pub request_tools: Option<String>,
    pub response_content: Option<String>,
    pub response_tool_calls: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub latency_ms: Option<i64>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Builder for creating a trace - start before the LLM call, complete after
#[derive(Debug, Clone)]
pub struct TraceBuilder {
    pub id: String,
    pub session_id: String,
    pub thread_id: Option<String>,
    pub prompt_id: Option<String>,
    pub agent_role: AgentRole,
    pub model_provider: String,
    pub model_id: String,
    pub template_inputs: Option<String>,
    pub rendered_prompt: Option<String>,
    pub request_messages: String,
    pub request_tools: Option<String>,
    pub started_at: DateTime<Utc>,
}

impl TraceBuilder {
    pub fn new(
        session_id: String,
        agent_role: AgentRole,
        model_provider: String,
        model_id: String,
        request_messages: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            thread_id: None,
            prompt_id: None,
            agent_role,
            model_provider,
            model_id,
            template_inputs: None,
            rendered_prompt: None,
            request_messages,
            request_tools: None,
            started_at: Utc::now(),
        }
    }

    pub fn with_thread_id(mut self, thread_id: String) -> Self {
        self.thread_id = Some(thread_id);
        self
    }

    pub fn with_prompt(mut self, prompt_id: String, template_inputs: String, rendered: String) -> Self {
        self.prompt_id = Some(prompt_id);
        self.template_inputs = Some(template_inputs);
        self.rendered_prompt = Some(rendered);
        self
    }

    pub fn with_tools(mut self, tools: String) -> Self {
        self.request_tools = Some(tools);
        self
    }

    pub fn complete(
        self,
        response_content: Option<String>,
        response_tool_calls: Option<String>,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        total_tokens: Option<i64>,
    ) -> Trace {
        let completed_at = Utc::now();
        let latency_ms = (completed_at - self.started_at).num_milliseconds();

        Trace {
            id: self.id,
            session_id: self.session_id,
            thread_id: self.thread_id,
            prompt_id: self.prompt_id,
            agent_role: self.agent_role,
            model_provider: self.model_provider,
            model_id: self.model_id,
            template_inputs: self.template_inputs,
            rendered_prompt: self.rendered_prompt,
            request_messages: self.request_messages,
            request_tools: self.request_tools,
            response_content,
            response_tool_calls,
            input_tokens,
            output_tokens,
            total_tokens,
            latency_ms: Some(latency_ms),
            started_at: self.started_at,
            completed_at: Some(completed_at),
            error: None,
        }
    }

    pub fn fail(self, error: String) -> Trace {
        let completed_at = Utc::now();
        let latency_ms = (completed_at - self.started_at).num_milliseconds();

        Trace {
            id: self.id,
            session_id: self.session_id,
            thread_id: self.thread_id,
            prompt_id: self.prompt_id,
            agent_role: self.agent_role,
            model_provider: self.model_provider,
            model_id: self.model_id,
            template_inputs: self.template_inputs,
            rendered_prompt: self.rendered_prompt,
            request_messages: self.request_messages,
            request_tools: self.request_tools,
            response_content: None,
            response_tool_calls: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            latency_ms: Some(latency_ms),
            started_at: self.started_at,
            completed_at: Some(completed_at),
            error: Some(error),
        }
    }
}
