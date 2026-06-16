use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolError};

#[derive(Deserialize)]
struct DdgResponse {
    #[serde(rename = "AbstractText")]
    abstract_text: String,
    #[serde(rename = "AbstractURL")]
    abstract_url: String,
    #[serde(rename = "RelatedTopics")]
    related_topics: Vec<DdgTopic>,
}

#[derive(Deserialize)]
struct DdgTopic {
    #[serde(rename = "Text")]
    text: Option<String>,
    #[serde(rename = "FirstURL")]
    first_url: Option<String>,
}

pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "web_search(query: string) — search the web and return a summary of top results"
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'query'".into()))?;

        let encoded = query.replace(' ', "+");
        let url = format!(
            "https://api.duckduckgo.com/?q={encoded}&format=json&no_html=1&skip_disambig=1"
        );

        let resp: DdgResponse = ureq::get(&url)
            .call()
            .map_err(|e| ToolError::CommandFailed(format!("search request failed: {e}")))?
            .into_json()
            .map_err(|e| ToolError::CommandFailed(format!("bad search response: {e}")))?;

        let mut out = String::new();
        if !resp.abstract_text.is_empty() {
            out.push_str(&resp.abstract_text);
            if !resp.abstract_url.is_empty() {
                out.push_str(&format!("\nSource: {}", resp.abstract_url));
            }
            out.push('\n');
        }
        for topic in resp.related_topics.iter().take(5) {
            if let (Some(text), Some(url)) = (&topic.text, &topic.first_url) {
                out.push_str(&format!("- {text}\n  {url}\n"));
            }
        }
        if out.is_empty() {
            out = format!("No results found for: {query}");
        }
        Ok(out)
    }
}
