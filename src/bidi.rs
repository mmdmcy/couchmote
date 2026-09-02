use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

pub struct BidiClient {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl BidiClient {
    pub async fn connect(endpoint: &str) -> Result<Self> {
        let (stream, _) = connect_async(endpoint)
            .await
            .with_context(|| format!("failed to connect to Firefox BiDi at {endpoint}"))?;
        Ok(Self { stream, next_id: 1 })
    }

    pub async fn create_session(&mut self) -> Result<String> {
        self.send("session.new", json!({ "capabilities": {} }))
            .await?;
        let tree = self.send("browsingContext.getTree", json!({})).await?;
        tree.pointer("/result/contexts/0/context")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("Firefox BiDi returned no browsing context"))
    }

    pub async fn navigate(&mut self, context: &str, url: &str) -> Result<()> {
        self.send(
            "browsingContext.navigate",
            json!({
                "context": context,
                "url": url,
                "wait": "complete"
            }),
        )
        .await
        .map(|_| ())
    }

    pub async fn traverse_history(&mut self, context: &str, delta: i8) -> Result<()> {
        self.send(
            "browsingContext.traverseHistory",
            json!({ "context": context, "delta": delta }),
        )
        .await
        .map(|_| ())
    }

    pub async fn evaluate_json<T>(&mut self, context: &str, script: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self
            .send(
                "script.evaluate",
                json!({
                    "expression": script,
                    "target": { "context": context },
                    "awaitPromise": true,
                    "resultOwnership": "none"
                }),
            )
            .await?;
        let remote_result = response
            .pointer("/result/result")
            .ok_or_else(|| anyhow!("Firefox BiDi returned no script result"))?;
        if remote_result.get("type").and_then(Value::as_str) == Some("exception") {
            return Err(anyhow!("Firefox page script raised an exception"));
        }
        let serialized = remote_result
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Firefox page script did not return JSON text"))?;
        serde_json::from_str(serialized)
            .with_context(|| "failed to decode JSON returned by Firefox page script")
    }

    pub async fn evaluate_text(&mut self, context: &str, script: &str) -> Result<String> {
        let response = self
            .send(
                "script.evaluate",
                json!({
                    "expression": script,
                    "target": { "context": context },
                    "awaitPromise": true,
                    "resultOwnership": "none"
                }),
            )
            .await?;
        response
            .pointer("/result/result/value")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("Firefox page script returned no text"))
    }

    pub async fn key_press(&mut self, context: &str, keys: Vec<KeyStroke>) -> Result<()> {
        let actions = keys
            .into_iter()
            .map(|stroke| {
                if stroke.down_up {
                    json!([
                        { "type": "keyDown", "value": stroke.value },
                        { "type": "keyUp", "value": stroke.value }
                    ])
                } else {
                    json!([{ "type": "keyDown", "value": stroke.value }])
                }
            })
            .fold(Vec::new(), |mut values, value| {
                values.extend(value.as_array().cloned().unwrap_or_default());
                values
            });
        self.send(
            "input.performActions",
            json!({
                "context": context,
                "actions": [{ "type": "key", "id": "couchmote", "actions": actions }]
            }),
        )
        .await
        .map(|_| ())
    }

    pub async fn release_actions(&mut self, context: &str) -> Result<()> {
        self.send("input.releaseActions", json!({ "context": context }))
            .await
            .map(|_| ())
    }

    pub async fn activate(&mut self, context: &str) -> Result<()> {
        self.send("browsingContext.activate", json!({ "context": context }))
            .await
            .map(|_| ())
    }

    async fn send(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.stream
            .send(Message::Text(
                serde_json::to_string(&json!({
                    "id": id,
                    "method": method,
                    "params": params
                }))
                .expect("BiDi request should serialize")
                .into(),
            ))
            .await
            .context("failed to send Firefox BiDi command")?;

        while let Some(message) = self.stream.next().await {
            let message = message.context("failed to read Firefox BiDi response")?;
            let Message::Text(text) = message else {
                continue;
            };
            let response: Value =
                serde_json::from_str(&text).context("Firefox BiDi returned invalid JSON")?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if response.get("type").and_then(Value::as_str) == Some("error") {
                let error = response
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown Firefox BiDi error");
                return Err(anyhow!("Firefox BiDi {method} failed: {error}"));
            }
            return Ok(response);
        }

        Err(anyhow!(
            "Firefox BiDi connection closed while running {method}"
        ))
    }
}

#[derive(Debug, Clone)]
pub struct KeyStroke {
    pub value: String,
    pub down_up: bool,
}

impl KeyStroke {
    pub fn press(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            down_up: true,
        }
    }
}
