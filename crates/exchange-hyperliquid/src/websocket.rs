use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub struct HyperliquidWebSocket {
    ws_url: String,
    stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl HyperliquidWebSocket {
    /// Creates a new Hyperliquid WebSocket client
    #[must_use]
    pub const fn new(ws_url: String) -> Self {
        Self {
            ws_url,
            stream: None,
        }
    }

    /// Connects to the WebSocket server
    ///
    /// # Errors
    /// Returns error if connection fails or server is unreachable
    pub async fn connect(&mut self) -> Result<()> {
        let (ws_stream, _) = connect_async(&self.ws_url)
            .await
            .context("Failed to connect to WebSocket")?;
        self.stream = Some(ws_stream);
        tracing::info!("WebSocket connected to {}", self.ws_url);
        Ok(())
    }

    /// Subscribes to market data updates
    ///
    /// # Errors
    /// Returns error if WebSocket is not connected or send fails
    pub async fn subscribe(&mut self, subscription: serde_json::Value) -> Result<()> {
        if let Some(stream) = &mut self.stream {
            let msg = Message::Text(subscription.to_string());
            stream.send(msg).await?;
            Ok(())
        } else {
            anyhow::bail!("WebSocket not connected")
        }
    }

    /// Receives the next message from the WebSocket
    ///
    /// # Errors
    /// Returns error if WebSocket is not connected or receive fails
    pub async fn next_message(&mut self) -> Result<Option<serde_json::Value>> {
        if let Some(stream) = &mut self.stream {
            if let Some(msg) = stream.next().await {
                match msg? {
                    Message::Text(text) => {
                        let json: serde_json::Value = serde_json::from_str(&text)?;
                        Ok(Some(json))
                    }
                    Message::Close(_) => {
                        tracing::warn!("WebSocket closed, reconnecting...");
                        self.reconnect().await?;
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            } else {
                Ok(None)
            }
        } else {
            anyhow::bail!("WebSocket not connected")
        }
    }

    async fn reconnect(&mut self) -> Result<()> {
        self.stream = None;
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        self.connect().await
    }
}
