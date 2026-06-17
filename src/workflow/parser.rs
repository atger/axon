use color_eyre::eyre::{Result, bail, eyre};

pub struct WorkflowDef {
    pub name: String,
    pub description: Option<String>,
    pub raw_steps: String,
}

pub fn parse_workflow_md(raw: &str) -> Result<WorkflowDef> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        bail!("workflow file must start with `---` frontmatter");
    }

    let after_open = trimmed.find('\n').map(|i| &trimmed[i + 1..]).unwrap_or("");
    let close = after_open
        .find("\n---")
        .ok_or_else(|| eyre!("workflow file is missing the closing `---`"))?;

    let frontmatter = &after_open[..close];
    let body = after_open[close..].trim_start_matches("\n---").trim_start();

    let mut name = String::new();
    let mut description = None;
    for line in frontmatter.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().trim_matches('"').trim_matches('\'').to_string();
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }

    if name.is_empty() {
        bail!("workflow frontmatter is missing `name:`");
    }

    Ok(WorkflowDef {
        name,
        description,
        raw_steps: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_workflow() {
        let raw = "---\nname: my-workflow\ndescription: A test\n---\n\n1. Step one\n2. Step two\n";
        let def = parse_workflow_md(raw).unwrap();
        assert_eq!(def.name, "my-workflow");
        assert_eq!(def.description.as_deref(), Some("A test"));
        assert!(def.raw_steps.contains("Step one"));
    }

    #[test]
    fn missing_frontmatter_errors() {
        let raw = "# Just a heading\n1. Do something";
        assert!(parse_workflow_md(raw).is_err());
    }

    #[test]
    fn missing_name_errors() {
        let raw = "---\ndescription: No name\n---\n1. Step";
        assert!(parse_workflow_md(raw).is_err());
    }
}
