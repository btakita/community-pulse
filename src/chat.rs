use crate::domain::MAX_BUDGET;
use crate::tools::ToolBridge;
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::env;
use std::sync::{Arc, Mutex};

const SYSTEM_PROMPT: &str = r#"You are the concise host of Community Pulse.
Use tools instead of guessing about trends or user state. Keep narration to two
short sentences unless the user asks for detail. The digest always has a
user-owned attention budget, defaults to five, and never exceeds ten. Use
set_interests to change that budget. Explain ranking using velocity, baseline
z-score, source evidence, and the user's explicit interests."#;

#[derive(Debug, Clone)]
pub enum ChatEvent {
    Delta(String),
    ToolCall { name: String, result: Value },
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
}

impl AgentConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = env::var("PULSE_API_KEY")
            .or_else(|_| env::var("OPENAI_API_KEY"))
            .context("set PULSE_API_KEY or use --replay")?;
        Ok(Self {
            api_base: env::var("PULSE_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned()),
            api_key,
            model: env::var("PULSE_MODEL").unwrap_or_else(|_| "gpt-4.1-mini".to_owned()),
        })
    }
}

#[derive(Clone)]
pub struct ChatSession {
    mode: ChatMode,
    bridge: ToolBridge,
}

#[derive(Clone)]
enum ChatMode {
    Live {
        config: AgentConfig,
        client: Client,
        history: Arc<Mutex<Vec<Value>>>,
    },
    Replay,
}

impl ChatSession {
    pub fn live(config: AgentConfig, bridge: ToolBridge) -> Result<Self> {
        Ok(Self {
            mode: ChatMode::Live {
                config,
                client: Client::builder()
                    .user_agent("community-pulse/0.1")
                    .build()?,
                history: Arc::new(Mutex::new(vec![
                    json!({ "role": "system", "content": SYSTEM_PROMPT }),
                ])),
            },
            bridge,
        })
    }

    pub fn replay(bridge: ToolBridge) -> Self {
        Self {
            mode: ChatMode::Replay,
            bridge,
        }
    }

    pub async fn respond<F>(&self, user: &str, mut emit: F) -> Result<()>
    where
        F: FnMut(ChatEvent) + Send,
    {
        match &self.mode {
            ChatMode::Live {
                config,
                client,
                history,
            } => {
                self.respond_live(config, client, history, user, &mut emit)
                    .await
            }
            ChatMode::Replay => self.respond_replay(user, &mut emit),
        }
    }

    async fn respond_live<F>(
        &self,
        config: &AgentConfig,
        client: &Client,
        history: &Arc<Mutex<Vec<Value>>>,
        user: &str,
        emit: &mut F,
    ) -> Result<()>
    where
        F: FnMut(ChatEvent) + Send,
    {
        history
            .lock()
            .expect("chat history lock poisoned")
            .push(json!({ "role": "user", "content": user }));

        for _ in 0..4 {
            let messages = history.lock().expect("chat history lock poisoned").clone();
            let endpoint = format!("{}/chat/completions", config.api_base.trim_end_matches('/'));
            let response = client
                .post(endpoint)
                .bearer_auth(&config.api_key)
                .json(&json!({
                    "model": config.model,
                    "stream": true,
                    "messages": messages,
                    "tools": ToolBridge::tool_definitions(),
                    "tool_choice": "auto"
                }))
                .send()
                .await?
                .error_for_status()
                .context("chat completion request failed")?;
            let turn = read_sse_turn(response, emit).await?;

            if turn.tool_calls.is_empty() {
                history
                    .lock()
                    .expect("chat history lock poisoned")
                    .push(json!({ "role": "assistant", "content": turn.content }));
                return Ok(());
            }

            let assistant_calls = turn
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.name, "arguments": call.arguments }
                    })
                })
                .collect::<Vec<_>>();
            history
                .lock()
                .expect("chat history lock poisoned")
                .push(json!({
                    "role": "assistant",
                    "content": if turn.content.is_empty() { Value::Null } else { Value::String(turn.content) },
                    "tool_calls": assistant_calls
        }));

            for call in turn.tool_calls {
                let result = match self.bridge.call(&call.name, &call.arguments) {
                    Ok(result) => result,
                    Err(error) => json!({ "error": format!("{error:#}") }),
                };
                emit(ChatEvent::ToolCall {
                    name: call.name.clone(),
                    result: result.clone(),
                });
                history
                    .lock()
                    .expect("chat history lock poisoned")
                    .push(json!({
                        "role": "tool",
                        "tool_call_id": call.id,
                        "content": serde_json::to_string(&result)?
                    }));
            }
        }
        bail!("agent exceeded the four-round tool-call limit")
    }

    fn respond_replay<F>(&self, user: &str, emit: &mut F) -> Result<()>
    where
        F: FnMut(ChatEvent) + Send,
    {
        let lower = user.to_lowercase();
        let requested_budget = inferred_budget(&lower);
        let (name, arguments, narration) = if lower.contains("track") || lower.contains("subscribe")
        {
            let topic = inferred_topic(&lower).unwrap_or("wasm-runtimes");
            (
                "subscribe_topic",
                json!({ "topic": topic }),
                format!(
                    "I added {topic} to your tracked topics. The same subscription can become a personal Spark alert later."
                ),
            )
        } else if lower.contains("why") || lower.contains("explain") {
            let topic = inferred_topic(&lower)
                .map(str::to_owned)
                .or_else(|| {
                    self.bridge
                        .snapshot()
                        .digest
                        .first()
                        .map(|card| card.id.clone())
                })
                .unwrap_or_else(|| "rust".to_owned());
            (
                "explain_trend",
                json!({ "id": topic }),
                "The evidence view now shows recent velocity, its seven-day baseline, and the source posts behind the score.".to_owned(),
            )
        } else if requested_budget.is_some()
            || lower.contains("more ")
            || lower.contains("less ")
            || lower.contains("mute")
        {
            let mut add = Vec::new();
            let mut remove = Vec::new();
            if lower.contains("rust") {
                if lower.contains("less rust") || lower.contains("mute rust") {
                    remove.push("rust");
                } else {
                    add.push("rust");
                }
            }
            if lower.contains("crypto") {
                if lower.contains("more crypto") {
                    add.push("crypto");
                } else {
                    remove.push("crypto");
                }
            }
            if lower.contains("local") {
                add.push("local-first");
            }
            if lower.contains("ai") {
                add.push("ai-infra");
            }
            let arguments = if let Some(budget) = requested_budget {
                json!({ "add": add, "remove": remove, "attention_budget": budget })
            } else {
                json!({ "add": add, "remove": remove })
            };
            let narration = if let Some(budget) = requested_budget {
                let budget = budget.clamp(1, MAX_BUDGET);
                format!(
                    "I set your attention budget to {budget} and reranked the pulse immediately. Evidence and source exploration stay available beyond the digest."
                )
            } else {
                "I updated the interest mixer and reranked the budgeted digest immediately."
                    .to_owned()
            };
            ("set_interests", arguments, narration)
        } else {
            (
                "get_pulse",
                json!({}),
                "The pulse stays inside your attention budget. WASM and Rust are moving fastest in the fixture, with local-first and AI infrastructure close behind.".to_owned(),
            )
        };

        let result = self.bridge.call(name, &arguments.to_string())?;
        emit(ChatEvent::ToolCall {
            name: name.to_owned(),
            result,
        });
        for piece in narration.split_inclusive(' ') {
            emit(ChatEvent::Delta(piece.to_owned()));
        }
        Ok(())
    }
}

fn inferred_topic(input: &str) -> Option<&'static str> {
    [
        ("wasm", "wasm-runtimes"),
        ("webassembly", "wasm-runtimes"),
        ("local", "local-first"),
        ("rust", "rust"),
        ("crypto", "crypto"),
        ("database", "databases"),
        ("sqlite", "databases"),
        ("privacy", "privacy"),
        ("ai", "ai-infra"),
    ]
    .into_iter()
    .find_map(|(needle, topic)| input.contains(needle).then_some(topic))
}

fn inferred_budget(input: &str) -> Option<usize> {
    if !input.contains("budget") && !input.contains("give me") && !input.contains("show me") {
        return None;
    }
    input
        .split(|character: char| !character.is_ascii_alphanumeric())
        .find_map(|word| match word {
            "one" => Some(1),
            "two" => Some(2),
            "three" => Some(3),
            "four" => Some(4),
            "five" => Some(5),
            "six" => Some(6),
            "seven" => Some(7),
            "eight" => Some(8),
            "nine" => Some(9),
            "ten" => Some(10),
            _ => word.parse().ok(),
        })
}

#[derive(Debug, Default)]
struct AssistantTurn {
    content: String,
    tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Default)]
struct ToolCall {
    id: String,
    name: String,
    arguments: String,
}

async fn read_sse_turn<F>(response: reqwest::Response, emit: &mut F) -> Result<AssistantTurn>
where
    F: FnMut(ChatEvent) + Send,
{
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    let mut content = String::new();
    let mut calls = BTreeMap::<usize, ToolCall>::new();

    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk?);
        while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let line = buffer.drain(..=newline).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line)?.trim();
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" || data.is_empty() {
                continue;
            }
            let chunk: CompletionChunk = serde_json::from_str(data)
                .with_context(|| format!("decode chat SSE event: {data}"))?;
            for choice in chunk.choices {
                if let Some(delta) = choice.delta.content {
                    content.push_str(&delta);
                    emit(ChatEvent::Delta(delta));
                }
                for call in choice.delta.tool_calls {
                    let target = calls.entry(call.index).or_default();
                    if let Some(id) = call.id {
                        target.id = id;
                    }
                    if let Some(function) = call.function {
                        if let Some(name) = function.name {
                            target.name.push_str(&name);
                        }
                        if let Some(arguments) = function.arguments {
                            target.arguments.push_str(&arguments);
                        }
                    }
                }
            }
        }
    }

    let tool_calls = calls
        .into_iter()
        .map(|(index, mut call)| {
            if call.id.is_empty() {
                call.id = format!("pulse-call-{index}");
            }
            call
        })
        .collect();
    Ok(AssistantTurn {
        content,
        tool_calls,
    })
}

#[derive(Deserialize)]
struct CompletionChunk {
    #[serde(default)]
    choices: Vec<CompletionChoice>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    delta: CompletionDelta,
}

#[derive(Deserialize)]
struct CompletionDelta {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<CompletionToolCall>,
}

#[derive(Deserialize)]
struct CompletionToolCall {
    index: usize,
    id: Option<String>,
    function: Option<CompletionFunction>,
}

#[derive(Deserialize)]
struct CompletionFunction {
    name: Option<String>,
    arguments: Option<String>,
}
