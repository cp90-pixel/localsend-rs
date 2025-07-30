use std::{collections::HashMap, path::Path};

use reqwest::{Client as ReqwestClient, ClientBuilder, StatusCode};
use tokio::fs;
use tracing::{debug, info};
use uuid::Uuid;

use crate::{DeviceInfo, FileInfo, FileType};

pub struct Client {
    http_client: ReqwestClient,
    device_info: DeviceInfo,
}

impl Client {
    pub fn new(device_info: DeviceInfo) -> Self {
        let http_client = ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        Self {
            http_client,
            device_info,
        }
    }

    pub async fn send_files(
        &self,
        target_device: &DeviceInfo,
        file_paths: Vec<&Path>,
    ) -> Result<(), String> {
        // Prepare file info for each file
        let mut files = HashMap::new();
        
        for path in &file_paths {
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
        
        let response = self.http_client
            .post(&target_addr)
            .json(&send_request)
            .send()
            .await
            .map_err(|e| format!("Failed to send request: {}", e))?;
        
        let status = response.status();
        if status != StatusCode::OK {
            let error_text = response.text().await.unwrap_or_default();
            return Err(format!("Request failed with status {}: {}", status, error_text));
        }
        
        // Parse response to get file tokens
        let tokens: HashMap<String, String> = response.json().await
            .map_err(|e| format!("Failed to parse response: {}", e))?;
        
        debug!("Received tokens: {:?}", tokens);
        
        // Send each file
        for (idx, (file_id, token)) in tokens.iter().enumerate() {
            let path = &file_paths[idx];
            let file_url = format!(
                "https://{}:{}/api/localsend/v1/send?fileId={}&token={}",
                target_device.ip, target_device.port, file_id, token
            );
            
            info!("Sending file: {}", path.display());
            
            // Read file content
            let file_content = fs::read(path).await
                .map_err(|e| format!("Failed to read file {}: {}", path.display(), e))?;
            
            // Send file
            let response = self.http_client
                .post(&file_url)
                .body(file_content)
                .send()
                .await
                .map_err(|e| format!("Failed to send file {}: {}", path.display(), e))?;
            
            let status = response.status();
            if status != StatusCode::OK {
                let error_text = response.text().await.unwrap_or_default();
                return Err(format!("File transfer failed with status {}: {}", status, error_text));
            }
            
            info!("Successfully sent file: {}", path.display());
        }
        
        Ok(())
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