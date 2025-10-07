use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub struct HyperliquidWebSocket {
    ws_url: String,
    stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    last_ping: std::time::Instant,
}

impl HyperliquidWebSocket {
    /// Creates a new Hyperliquid WebSocket client
    #[must_use]
    pub fn new(ws_url: String) -> Self {
        Self {
            ws_url,
            stream: None,
            last_ping: std::time::Instant::now(),
        }
    }

    /// Connects to the WebSocket server
    ///
    /// # Errors
    /// Returns error if connection fails or server is unreachable
    pub async fn connect(&mut self) -> Result<()> {
        tracing::debug!("Attempting WebSocket connection to: {}", self.ws_url);

        let (ws_stream, response) = connect_async(&self.ws_url).await
            .map_err(|e| {
                tracing::error!("WebSocket connection error: {}", e);
                tracing::debug!("WebSocket error details: {:?}", e);
                anyhow::anyhow!("Failed to connect to WebSocket at {}: {}", self.ws_url, e)
            })?;

        self.stream = Some(ws_stream);
        tracing::info!("WebSocket connected to {} (HTTP status: {})", self.ws_url, response.status());
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
        // Send ping if it's been more than 50 seconds since last ping
        if self.last_ping.elapsed() > std::time::Duration::from_secs(50) {
            self.send_ping().await?;
            self.last_ping = std::time::Instant::now();
        }

        let stream = self.stream.as_mut()
            .ok_or_else(|| anyhow::anyhow!("WebSocket not connected"))?;

        if let Some(msg) = stream.next().await {
            self.handle_message(msg?).await
        } else {
            Ok(None)
        }
    }

    /// Handle different WebSocket message types
    async fn handle_message(&mut self, msg: Message) -> Result<Option<serde_json::Value>> {
        match msg {
            Message::Text(text) => {
                let json: serde_json::Value = serde_json::from_str(&text)?;
                Ok(Some(json))
            }
            Message::Ping(_) => {
                tracing::trace!("Received ping from server");
                Ok(None)
            }
            Message::Pong(_) => {
                tracing::trace!("Received pong from server");
                Ok(None)
            }
            Message::Close(_) => {
                tracing::warn!("WebSocket closed, reconnecting...");
                self.reconnect().await?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Sends a ping message to keep the connection alive
    ///
    /// # Errors
    /// Returns error if WebSocket is not connected or send fails
    async fn send_ping(&mut self) -> Result<()> {
        if let Some(stream) = &mut self.stream {
            let ping = serde_json::json!({"method": "ping"});
            let msg = Message::Text(ping.to_string());
            stream.send(msg).await?;
            tracing::trace!("Sent ping to server");
            Ok(())
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
