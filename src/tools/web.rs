use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError};

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "web_search(query: string) — search the web; use for current events, news, or anything you are unsure about"
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'query'".into()))?;

        let encoded = query.replace(' ', "+");
        let url = format!("https://html.duckduckgo.com/html/?q={encoded}");

        let html = ureq::get(&url)
            .set("User-Agent", "Mozilla/5.0 (compatible; axon/0.1)")
            .call()
            .map_err(|e| ToolError::CommandFailed(format!("search request failed: {e}")))?
            .into_string()
            .map_err(|e| ToolError::CommandFailed(format!("bad search response: {e}")))?;

        let snippets = extract_snippets(&html, 5);
        if snippets.is_empty() {
            return Ok(format!("No results found for: {query}"));
        }
        Ok(snippets.join("\n"))
    }
}

/// Extract up to `limit` result snippets from DDG HTML search results.
fn extract_snippets(html: &str, limit: usize) -> Vec<String> {
    let mut snippets = Vec::new();
    let marker = "class=\"result__snippet\"";
    let mut remaining = html;
    while snippets.len() < limit {
        let Some(pos) = remaining.find(marker) else {
            break;
        };
        remaining = &remaining[pos + marker.len()..];
        let Some(gt) = remaining.find('>') else {
            break;
        };
        remaining = &remaining[gt + 1..];
        let Some(end) = remaining.find("</a>") else {
            break;
        };
        let text = html_clean(remaining[..end].trim());
        if !text.is_empty() {
            snippets.push(text);
        }
    }
    snippets
}

/// Strip HTML tags and unescape common entities.
fn html_clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}
