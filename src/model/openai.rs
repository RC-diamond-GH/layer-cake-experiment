use super::{FinishReason, GenerateParams, GenerateResponse, ModelClient, ModelError, Usage};
use crate::context::{ChatMessage, LLMContext, ToolCall};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::time::Duration;

/// Credentials live only in this adapter, never in serializable request state.
pub struct OpenAiClient {
    endpoint: reqwest::Url,
    api_key: String,
    client: reqwest::Client,
}
impl OpenAiClient {
    pub fn new(endpoint: String, api_key: String) -> Result<Self, ModelError> {
        let endpoint = reqwest::Url::parse(&endpoint)
            .map_err(|_| ModelError::new("configuration", "invalid chat/completions URL"))?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ModelError::new(
                "configuration",
                "endpoint must be an HTTP(S) URL without credentials or fragment",
            ));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ModelError::new("configuration", "could not initialize HTTP client"))?;
        Ok(Self {
            endpoint,
            api_key,
            client,
        })
    }

    fn redact(&self, text: &str) -> String {
        if self.api_key.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.api_key, "[REDACTED]")
        }
    }
}

fn request(context: &LLMContext, params: &GenerateParams) -> Result<Value, ModelError> {
    if params.model.is_empty()
        || params.temperature.is_some_and(|t| !t.is_finite())
        || params.max_tokens == Some(0)
    {
        return Err(ModelError::new(
            "request",
            "model must be nonempty, temperature finite, and max_tokens positive",
        ));
    }
    let mut messages = Vec::with_capacity(context.messages.len() + context.global_prompt.len() + 1);
    messages.push(json!({"role":"system","content":context.system_prompt}));
    for global in &context.global_prompt {
        messages.push(
            json!({"role":"system","content":format!("{}\n{}", global.filename, global.content)}),
        );
    }
    for message in &context.messages {
        messages.push(match message {
            ChatMessage::User { content } => json!({"role":"user","content":content}),
            ChatMessage::Tool { tool_call_id, content } => json!({"role":"tool","tool_call_id":tool_call_id,"content":content}),
            ChatMessage::Assistant { content, tool_calls } => {
                let mut value = json!({"role":"assistant","content":content});
                if !tool_calls.is_empty() {
                    value["tool_calls"] = Value::Array(tool_calls.iter().map(|call| json!({
                        "id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments.to_string()}
                    })).collect());
                }
                value
            }
        });
    }
    let mut value = json!({"model":params.model,"messages":messages,"stream":false});
    if let Some(temperature) = params.temperature {
        value["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = params.max_tokens {
        value["max_tokens"] = json!(max_tokens);
    }
    if !context.tools.is_empty() {
        value["tools"] = Value::Array(
            context
                .tools
                .iter()
                .map(|tool| {
                    json!({"type":"function","function":{
                        "name":tool.name,"description":tool.description,"parameters":tool.parameters
                    }})
                })
                .collect(),
        );
    }
    Ok(value)
}

fn malformed(message: impl Into<String>) -> ModelError {
    ModelError::new("response_format", message)
}
fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str, ModelError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(format!("missing or invalid {field}")))
}
fn parse(value: &Value) -> Result<GenerateResponse, ModelError> {
    if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
        return Err(ModelError::new(
            "api",
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("provider returned an error"),
        ));
    }
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| malformed("response has no choices"))?;
    let message = choice
        .get("message")
        .ok_or_else(|| malformed("choice has no message"))?;
    if string_field(message, "role")? != "assistant" {
        return Err(malformed("choice message is not assistant"));
    }
    let content = match message.get("content") {
        None | Some(Value::Null) => None,
        Some(Value::String(content)) => Some(content.clone()),
        _ => return Err(malformed("assistant content must be a string or null")),
    };
    let calls = match message.get("tool_calls") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(calls)) => calls.as_slice(),
        _ => return Err(malformed("tool_calls must be an array")),
    };
    let mut tool_calls = Vec::with_capacity(calls.len());
    for call in calls {
        if string_field(call, "type")? != "function" {
            return Err(malformed("unsupported tool call type"));
        }
        let id = string_field(call, "id")?;
        let function = call
            .get("function")
            .ok_or_else(|| malformed("tool call has no function"))?;
        let name = string_field(function, "name")?;
        if id.is_empty() || name.is_empty() || tool_calls.iter().any(|c: &ToolCall| c.id == id) {
            return Err(malformed(
                "tool call ids and names must be nonempty, with unique ids",
            ));
        }
        let arguments = match function.get("arguments") {
            Some(Value::String(arguments)) => serde_json::from_str(arguments)
                .map_err(|e| malformed(format!("invalid tool arguments JSON: {e}")))?,
            Some(Value::Object(arguments)) => Value::Object(arguments.clone()),
            _ => {
                return Err(malformed(
                    "tool arguments must be a JSON object or encoded JSON object",
                ));
            }
        };
        if !arguments.is_object() {
            return Err(malformed("tool arguments must decode to an object"));
        }
        tool_calls.push(ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        });
    }
    let finish_reason = match choice.get("finish_reason") {
        Some(Value::String(reason)) => match reason.as_str() {
            "stop" => FinishReason::Stop,
            "tool_calls" | "function_call" => FinishReason::ToolCalls,
            "length" => FinishReason::Length,
            "content_filter" => FinishReason::ContentFilter,
            "error" => FinishReason::Error,
            other => FinishReason::Unknown(other.into()),
        },
        Some(Value::Null) => FinishReason::Unknown("null".into()),
        _ => return Err(malformed("missing or invalid finish_reason")),
    };
    let usage = match value.get("usage") {
        None | Some(Value::Null) => None,
        Some(usage) => {
            let number = |field| {
                usage
                    .get(field)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| malformed(format!("invalid usage.{field}")))
            };
            Some(Usage {
                input_tokens: number("prompt_tokens")?,
                output_tokens: number("completion_tokens")?,
                total_tokens: number("total_tokens")?,
            })
        }
    };
    Ok(GenerateResponse {
        message: ChatMessage::Assistant {
            content,
            tool_calls,
        },
        finish_reason,
        usage,
    })
}

#[async_trait]
impl ModelClient for OpenAiClient {
    async fn generate(
        &self,
        context: &LLMContext,
        params: &GenerateParams,
    ) -> Result<GenerateResponse, ModelError> {
        let body = request(context, params)?;
        let mut request = self.client.post(self.endpoint.clone()).json(&body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        let response = request.send().await.map_err(|error| {
            ModelError::new(
                if error.is_timeout() {
                    "timeout"
                } else {
                    "transport"
                },
                if error.is_timeout() {
                    "model HTTP request timed out"
                } else {
                    "model HTTP request failed"
                },
            )
        })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|_| ModelError::new("transport", "failed to read model HTTP response"))?;
        if !status.is_success() {
            let parsed = serde_json::from_slice::<Value>(&bytes).ok();
            let message = if bytes.is_empty() {
                "empty response body"
            } else if let Some(value) = &parsed {
                value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("request rejected")
            } else {
                "non-JSON response body"
            };
            let hint = if status == reqwest::StatusCode::NOT_FOUND {
                "; check OPENAI_BASE_URL or model.endpoint: the request must target /chat/completions under the API base path"
            } else {
                ""
            };
            return Err(ModelError::new(
                format!("http_{}", status.as_u16()),
                self.redact(&format!("model HTTP {status}: {message}{hint}")),
            ));
        }
        if bytes.is_empty() {
            return Err(ModelError::new(
                "response_format",
                format!("model HTTP {status}: empty response body"),
            ));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            ModelError::new(
                "response_format",
                format!("model HTTP {status}: response body is not valid JSON"),
            )
        })?;
        parse(&value).map_err(|error| ModelError::new(error.kind, self.redact(&error.message)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{GlobalPrompt, ToolDefinition};

    #[test]
    fn converts_global_tools_and_encoded_arguments_without_internal_metadata() {
        let mut context = LLMContext::new("private-context", "system");
        context.global_prompt.push(GlobalPrompt {
            filename: "resource.txt".into(),
            content: "fresh".into(),
        });
        context.set_tools(vec![ToolDefinition {
            name: "read".into(),
            description: "read".into(),
            parameters: json!({"type":"object"}),
        }]);
        context.append(ChatMessage::Assistant {
            content: None,
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: json!({"key":"x"}),
            }],
        });
        let value = request(&context, &GenerateParams::default()).expect("request conversion");
        assert_eq!(value["messages"][1]["content"], "resource.txt\nfresh");
        assert_eq!(value["tools"][0]["function"]["name"], "read");
        assert_eq!(
            value["messages"][2]["tool_calls"][0]["function"]["arguments"],
            "{\"key\":\"x\"}"
        );
        assert!(value.get("id").is_none());
    }

    #[test]
    fn parses_usage_unknown_finish_and_rejects_malformed_arguments() {
        let mut value = json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"read","arguments":"{\"key\":\"x\"}"}}]},"finish_reason":"future_reason"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}});
        let response = parse(&value).expect("valid response");
        assert_eq!(response.usage.expect("usage").total_tokens, 5);
        assert_eq!(
            response.finish_reason,
            FinishReason::Unknown("future_reason".into())
        );
        value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] = json!("{");
        assert_eq!(parse(&value).expect_err("bad JSON").kind, "response_format");
        assert!(parse(&json!({"choices":[]})).is_err());
        assert_eq!(
            parse(&json!({"error":{"message":"denied"}}))
                .expect_err("provider error")
                .kind,
            "api"
        );
    }

    #[tokio::test]
    async fn actual_http_boundary_encodes_and_parses_provider_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test HTTP listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let count = stream.read(&mut chunk).await.expect("read request");
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .expect("content length");
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let request = String::from_utf8(bytes).expect("UTF-8 request");
            assert!(request.contains("Bearer test-secret"));
            assert!(request.contains("\"model\":\"script\""));
            let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        let client = OpenAiClient::new(
            format!("http://{address}/v1/chat/completions"),
            "test-secret".into(),
        )
        .expect("client");
        let response = client
            .generate(&LLMContext::new("c", "s"), &GenerateParams::default())
            .await
            .expect("HTTP generation");
        assert_eq!(response.finish_reason, FinishReason::Stop);
        server.await.expect("HTTP test task");
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn endpoint_credentials_are_rejected_and_provider_error_secrets_are_redacted() {
        assert!(
            OpenAiClient::new(
                "https://user:password@example.test/chat/completions".into(),
                "secret".into()
            )
            .is_err()
        );
        let client = OpenAiClient::new(
            "https://example.test/chat/completions".into(),
            "private-api-key".into(),
        )
        .expect("client");
        assert_eq!(
            client.redact("invalid token private-api-key"),
            "invalid token [REDACTED]"
        );
    }
}
