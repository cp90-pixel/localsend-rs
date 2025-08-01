use std::{collections::HashMap, path::Path, time::Duration, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use reqwest::{Client as ReqwestClient, ClientBuilder, StatusCode};
use tokio_util::io::ReaderStream;
use tokio::{fs, time::timeout};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{DeviceInfo, FileInfo, FileType};

/// Callback trait for tracking file upload progress
pub trait ProgressCallback: Send + Sync {
    fn on_progress(&self, file_id: &str, bytes_sent: u64, total_bytes: u64);
    fn on_file_start(&self, file_id: &str, file_name: &str, total_bytes: u64);
    fn on_file_complete(&self, file_id: &str);
}

/// A wrapper around AsyncRead that tracks progress
struct ProgressReader<R> {
    inner: R,
    bytes_read: u64,
    total_bytes: u64,
    file_id: String,
    progress_callback: Option<Arc<dyn ProgressCallback>>,
}

impl<R> ProgressReader<R> {
    fn new(
        inner: R,
        total_bytes: u64,
        file_id: String,
        progress_callback: Option<Arc<dyn ProgressCallback>>,
    ) -> Self {
        Self {
            inner,
            bytes_read: 0,
            total_bytes,
            file_id,
            progress_callback,
        }
    }
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ProgressReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        
        if let std::task::Poll::Ready(Ok(())) = &result {
            let bytes_read_this_time = buf.filled().len() - before;
            self.bytes_read += bytes_read_this_time as u64;
            
            if let Some(callback) = &self.progress_callback {
                callback.on_progress(&self.file_id, self.bytes_read, self.total_bytes);
            }
        }
        
        result
    }
}

#[derive(Clone)]
pub struct Client {
    http_client: ReqwestClient,
    device_info: DeviceInfo,
    is_cancelled: Arc<AtomicBool>,
    progress_callback: Option<Arc<dyn ProgressCallback>>,
}

impl Client {
    pub fn new(device_info: DeviceInfo) -> Self {
        let http_client = ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap();

        Self {
            http_client,
            device_info,
            is_cancelled: Arc::new(AtomicBool::new(false)),
            progress_callback: None,
        }
    }

    pub fn with_progress_callback(mut self, callback: Arc<dyn ProgressCallback>) -> Self {
        self.progress_callback = Some(callback);
        self
    }

    pub fn cancel(&self) {
        self.is_cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Relaxed)
    }

    pub async fn send_files(
        &self,
        target_device: &DeviceInfo,
        file_paths: Vec<&Path>,
    ) -> Result<(), String> {
        if self.is_cancelled() {
            return Err("Operation was cancelled".to_string());
        }

        // Prepare file info for each file
        let mut files = HashMap::new();
        
        for path in &file_paths {
            if self.is_cancelled() {
                return Err("Operation was cancelled".to_string());
            }

            if !path.exists() {
                return Err(format!("File not found: {}", path.display()));
            }
            
            let file_id = Uuid::new_v4().to_string();
            let file_name = path.file_name().unwrap().to_string_lossy().to_string();
            let metadata = fs::metadata(path).await.map_err(|e| e.to_string())?;
            let size = metadata.len() as usize;
            
            // Determine file type based on extension
            let file_type = if let Some(ext) = path.extension() {
                match ext.to_string_lossy().to_lowercase().as_str() {
                    "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" => FileType::Image,
                    "mp4" | "webm" | "mkv" | "avi" | "mov" => FileType::Video,
                    "pdf" => FileType::Pdf,
                    "txt" | "md" | "rs" | "js" | "html" | "css" | "json" => FileType::Text,
                    _ => FileType::Other,
                }
            } else {
                FileType::Other
            };
            
            let file_info = FileInfo {
                id: file_id.clone(),
                size,
                file_name,
                file_type,
            };
            
            files.insert(file_id, file_info);
        }
        
        // Send request to target device
        let target_addr = format!(
            "https://{}:{}/api/localsend/v1/send-request",
            target_device.ip, target_device.port
        );
        
        let send_request = serde_json::json!({
            "info": self.device_info,
            "files": files,
        });
        
        debug!("Sending request: {:?}", send_request);
        
        // Send initial request with retry logic for connection errors
        let tokens = self.send_request_with_retry(&target_addr, &send_request, 3).await?;
        
        debug!("Received tokens: {:?}", tokens);
        
        // Send each file
        for (idx, (file_id, token)) in tokens.iter().enumerate() {
            if self.is_cancelled() {
                return Err("Operation was cancelled".to_string());
            }

            let path = &file_paths[idx];
            let file_url = format!(
                "https://{}:{}/api/localsend/v1/send?fileId={}&token={}",
                target_device.ip, target_device.port, file_id, token
            );
            
            info!("Sending file: {}", path.display());
            
            // Get file info for progress tracking
            let file_info = files.get(file_id).unwrap();
            
            // Notify progress callback that file is starting
            if let Some(callback) = &self.progress_callback {
                callback.on_file_start(file_id, &file_info.file_name, file_info.size as u64);
            }
            
            // Send file with retry logic for connection errors
            self.send_file_with_retry(&file_url, path, file_id, 3).await?;
            
            // Notify progress callback that file is complete
            if let Some(callback) = &self.progress_callback {
                callback.on_file_complete(file_id);
            }
            
            info!("Successfully sent file: {}", path.display());
        }
        
        Ok(())
    }

    async fn send_request_with_retry(
        &self,
        url: &str,
        request_body: &serde_json::Value,
        max_retries: u32,
    ) -> Result<HashMap<String, String>, String> {
        let mut last_error = String::new();
        
        for attempt in 0..=max_retries {
            if self.is_cancelled() {
                return Err("Operation was cancelled".to_string());
            }

            let result = timeout(
                Duration::from_secs(30),
                self.http_client
                    .post(url)
                    .json(request_body)
                    .send()
            ).await;

            match result {
                Ok(Ok(response)) => {
                    let status = response.status();
                    if status == StatusCode::OK {
                        match response.json::<HashMap<String, String>>().await {
                            Ok(tokens) => return Ok(tokens),
                            Err(e) => {
                                last_error = format!("Failed to parse response: {}", e);
                                if attempt == max_retries {
                                    break;
                                }
                            }
                        }
                    } else {
                        let error_text = response.text().await.unwrap_or_default();
                        last_error = format!("Request failed with status {}: {}", status, error_text);
                        if attempt == max_retries {
                            break;
                        }
                    }
                }
                Ok(Err(e)) => {
                    last_error = format!("Network error: {}", e);
                    if self.is_connection_error(&e) && attempt < max_retries {
                        warn!("Connection error on attempt {}, retrying...: {}", attempt + 1, e);
                        tokio::time::sleep(Duration::from_millis(1000 * (attempt + 1) as u64)).await;
                        continue;
                    }
                    if attempt == max_retries {
                        break;
                    }
                }
                Err(_) => {
                    last_error = "Request timed out".to_string();
                    if attempt == max_retries {
                        break;
                    }
                }
            }
            
            if attempt < max_retries {
                tokio::time::sleep(Duration::from_millis(500 * (attempt + 1) as u64)).await;
            }
        }
        
        Err(format!("Failed after {} retries: {}", max_retries, last_error))
    }

    async fn send_file_with_retry(
        &self,
        url: &str,
        path: &Path,
        file_id: &str,
        max_retries: u32,
    ) -> Result<(), String> {
        
        let mut last_error = String::new();
        let file_size = fs::metadata(path).await
            .map_err(|e| format!("Failed to get file metadata: {}", e))?
            .len();
        
        for attempt in 0..=max_retries {
            if self.is_cancelled() {
                return Err("Operation was cancelled".to_string());
            }

            // Open file for this attempt
            let file = match fs::File::open(path).await {
                Ok(f) => f,
                Err(e) => {
                    last_error = format!("Failed to open file {}: {}", path.display(), e);
                    break;
                }
            };
            
            // Create progress tracking reader
            let progress_reader = ProgressReader::new(
                file, 
                file_size,
                file_id.to_string(),
                self.progress_callback.clone()
            );
            
            let stream = ReaderStream::new(progress_reader);
            let body = reqwest::Body::wrap_stream(stream);

            let result = timeout(
                Duration::from_secs(60), // Longer timeout for file uploads
                self.http_client
                    .post(url)
                    .body(body)
                    .send()
            ).await;

            match result {
                Ok(Ok(response)) => {
                    let status = response.status();
                    if status == StatusCode::OK {
                        return Ok(());
                    } else {
                        let error_text = response.text().await.unwrap_or_default();
                        last_error = format!("File transfer failed with status {}: {}", status, error_text);
                        if attempt == max_retries {
                            break;
                        }
                    }
                }
                Ok(Err(e)) => {
                    last_error = format!("Network error sending file {}: {}", path.display(), e);
                    if self.is_connection_error(&e) && attempt < max_retries {
                        warn!("Connection error on attempt {} for file {}, retrying...: {}", 
                              attempt + 1, path.display(), e);
                        tokio::time::sleep(Duration::from_millis(2000 * (attempt + 1) as u64)).await;
                        continue;
                    }
                    if attempt == max_retries {
                        break;
                    }
                }
                Err(_) => {
                    last_error = format!("File transfer timed out for {}", path.display());
                    if attempt == max_retries {
                        break;
                    }
                }
            }
            
            if attempt < max_retries {
                tokio::time::sleep(Duration::from_millis(1000 * (attempt + 1) as u64)).await;
            }
        }
        
        Err(format!("Failed to send file {} after {} retries: {}", path.display(), max_retries, last_error))
    }

    fn is_connection_error(&self, error: &reqwest::Error) -> bool {
        error.is_connect() || 
        error.is_timeout() ||
        error.to_string().contains("connection reset") ||
        error.to_string().contains("broken pipe") ||
        error.to_string().contains("connection aborted") ||
        error.to_string().contains("network unreachable")
    }
    
    pub async fn cancel_session(&self, target_device: &DeviceInfo) -> Result<(), String> {
        let target_addr = format!(
            "https://{}:{}/api/localsend/v1/cancel",
            target_device.ip, target_device.port
        );
        
        let response = self.http_client
            .post(&target_addr)
            .send()
            .await
            .map_err(|e| format!("Failed to send cancel request: {}", e))?;
        
        let status = response.status();
        if status != StatusCode::OK {
            let error_text = response.text().await.unwrap_or_default();
            return Err(format!("Cancel request failed with status {}: {}", status, error_text));
        }
        
        Ok(())
    }
}