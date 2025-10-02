use crate::config::AppConfig;
use crate::config_loader::ConfigLoader;
use anyhow::Result;
use notify::{Event, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::watch;

pub struct ConfigWatcher {
    tx: watch::Sender<AppConfig>,
}

impl ConfigWatcher {
    /// Creates a new configuration watcher with an initial configuration.
    ///
    /// Returns a tuple of the watcher and a receiver for configuration updates.
    #[must_use]
    pub fn new(initial_config: AppConfig) -> (Self, watch::Receiver<AppConfig>) {
        let (tx, rx) = watch::channel(initial_config);
        (Self { tx }, rx)
    }

    /// Watches the configuration file for changes and broadcasts updates.
    ///
    /// # Errors
    ///
    /// Returns an error if file watching cannot be initiated or if the watcher task fails.
    pub async fn watch(&self, config_path: &str) -> Result<()> {
        let tx = self.tx.clone();
        let config_path = config_path.to_string();

        tokio::task::spawn_blocking(move || {
            let (notify_tx, notify_rx) = std::sync::mpsc::channel();

            let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
                if let Ok(event) = res {
                    let _ = notify_tx.send(event);
                }
            })?;

            watcher.watch(Path::new(&config_path), RecursiveMode::NonRecursive)?;

            for event in notify_rx {
                if event.kind.is_modify() {
                    tracing::info!("Config file changed, reloading...");
                    match ConfigLoader::load() {
                        Ok(new_config) => {
                            let _ = tx.send(new_config);
                            tracing::info!("Config reloaded successfully");
                        }
                        Err(e) => {
                            tracing::error!("Failed to reload config: {}", e);
                        }
                    }
                }
            }

            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }
}
