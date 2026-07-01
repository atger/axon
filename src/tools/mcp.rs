use std::sync::Arc;
use async_trait::async_trait;
use serde_json::Value;
use rust_mcp_sdk::{
    mcp_client::{client_runtime, ClientHandler, ClientRuntime, McpClientOptions},
    schema::*,
    McpClient, StdioTransport, ToMcpClientHandler, TransportOptions,
};

use super::{Tool, ToolError};

pub struct McpTool {
    name: String,
    description: String,
    client: Arc<ClientRuntime>,
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .finish()
    }
}

pub struct MyClientHandler;

#[async_trait]
impl ClientHandler for MyClientHandler {}

impl McpTool {
    pub fn new(name: String, description: String, client: Arc<ClientRuntime>) -> Self {
        Self { name, description, client }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn execute(&self, args: Value) -> std::result::Result<String, ToolError> {
        let mut params = CallToolRequestParams::new(self.name.clone());
        params.arguments = args.as_object().cloned();

        let response: CallToolResult = self.client.request_tool_call(params).await
            .map_err(|e| ToolError::CommandFailed(format!("MCP call failed: {e}")))?;
        
        // Extract text from CallToolResult
        let mut result = String::new();
        for content in response.content {
            match content {
                ContentBlock::TextContent(text_content) => {
                    if !result.is_empty() {
                        result.push_str("\n");
                    }
                    result.push_str(&text_content.text);
                }
                _ => {} // Ignore other content types for now
            }
        }
        
        if response.is_error.unwrap_or(false) {
            return Err(ToolError::CommandFailed(result));
        }
        
        Ok(result)
    }
}

pub async fn load_raw_mcp_tools(
    name: &str,
    command: &str,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
) -> std::result::Result<(Vec<McpTool>, Arc<ClientRuntime>), ToolError> {
    eprintln!("Connecting to MCP server: {name} ({command} {})...", args.join(" "));
    
    let client_info = Implementation {
        name: "axon".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: None,
        icons: vec![],
        title: None,
        website_url: None,
    };

    let transport = StdioTransport::create_with_server_launch(
        command,
        args.to_vec(),
        Some(env.clone()),
        TransportOptions::default(),
    ).map_err(|e| ToolError::CommandFailed(format!("Failed to start MCP server {name}: {e}")))?;

    let handler = MyClientHandler {};
    
    let client_details = InitializeRequestParams {
        capabilities: ClientCapabilities::default(),
        client_info: client_info.clone(),
        protocol_version: LATEST_PROTOCOL_VERSION.into(),
        meta: None,
    };

    let options = McpClientOptions {
        client_details,
        transport,
        handler: handler.to_mcp_client_handler(),
        task_store: None,
        server_task_store: None,
        message_observer: None,
    };
    let client = client_runtime::create_client(options);
    let client_arc = client.clone();
    
    client.start().await
        .map_err(|e| ToolError::CommandFailed(format!("Failed to start MCP client {name}: {e}")))?;

    let tool_list = client_arc.request_tool_list(None).await
        .map_err(|e| ToolError::CommandFailed(format!("Failed to list MCP tools for {name}: {e}")))?;

    let mut tools: Vec<McpTool> = Vec::new();
    eprintln!("Found {} tools from MCP server '{name}'", tool_list.tools.len());
    for tool in tool_list.tools {
        let desc = tool.description.unwrap_or_default();
        tools.push(McpTool::new(tool.name, desc, client_arc.clone()));
    }

    Ok((tools, client_arc))
}

pub async fn load_mcp_tools(
    name: &str,
    command: &str,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
) -> std::result::Result<(Vec<Box<dyn Tool>>, Arc<ClientRuntime>), ToolError> {
    let (raw, client) = load_raw_mcp_tools(name, command, args, env).await?;
    let tools = raw.into_iter().map(|t| Box::new(t) as Box<dyn Tool>).collect();
    Ok((tools, client))
}
