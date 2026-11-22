use crate::mcp::{MemoryServer, Options};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use serde::Serialize;
use std::{net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

const ADDRESS: &str = "127.0.0.1:37241";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpStatus {
    running: bool,
    url: String,
}

#[derive(Default)]
pub struct HttpRuntime(pub tokio::sync::Mutex<LocalHttpServer>);

#[derive(Default)]
pub struct LocalHttpServer {
    running: Option<RunningHttp>,
}

struct RunningHttp {
    address: SocketAddr,
    cancellation: CancellationToken,
    task: JoinHandle<Result<(), std::io::Error>>,
}

impl Drop for RunningHttp {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

impl LocalHttpServer {
    pub async fn status(&mut self) -> Result<HttpStatus, String> {
        if self
            .running
            .as_ref()
            .is_some_and(|server| server.task.is_finished())
        {
            let mut server = self.running.take().unwrap();
            (&mut server.task)
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("本地 MCP 服务已停止：{error}"))?;
        }
        Ok(HttpStatus {
            running: self.running.is_some(),
            url: format!(
                "http://{}/mcp",
                self.running
                    .as_ref()
                    .map(|server| server.address.to_string())
                    .unwrap_or_else(|| ADDRESS.into())
            ),
        })
    }

    pub async fn start(&mut self, options: Options) -> Result<HttpStatus, String> {
        self.start_at(ADDRESS, options).await
    }

    async fn start_at(&mut self, address: &str, options: Options) -> Result<HttpStatus, String> {
        let status = self.status().await?;
        if status.running {
            return Ok(status);
        }
        let listener = TcpListener::bind(address)
            .await
            .map_err(|error| format!("无法启动本地 MCP 服务（{address}）：{error}"))?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let cancellation = CancellationToken::new();
        let memory = MemoryServer::new(options);
        let service = StreamableHttpService::new(
            move || Ok(memory.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_allowed_origins([format!("http://{address}")])
                .with_cancellation_token(cancellation.child_token()),
        );
        let router = axum::Router::new().nest_service("/mcp", service);
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        self.running = Some(RunningHttp {
            address,
            cancellation,
            task,
        });
        self.status().await
    }

    pub async fn stop(&mut self) -> Result<HttpStatus, String> {
        if let Some(mut server) = self.running.take() {
            server.cancellation.cancel();
            (&mut server.task)
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("停止本地 MCP 服务失败：{error}"))?;
        }
        self.status().await
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tool_tests;
