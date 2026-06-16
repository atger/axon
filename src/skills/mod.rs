use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Context, Result, bail, eyre};

pub struct Skill {
    pub name: String,
    pub description: String,
    /// Body text of the SKILL.md with `{{SKILL_DIR}}` resolved to the skill's directory.
    pub content: String,
}

/// `~/.axon/skills/`, created on first access.
pub fn skills_dir() -> Result<PathBuf> {
    let dir = dirs::home_dir()
        .ok_or_else(|| eyre!("cannot determine home directory"))?
        .join(".axon")
        .join("skills");
    fs::create_dir_all(&dir).wrap_err("failed to create ~/.axon/skills")?;
    Ok(dir)
}

/// Parse a SKILL.md string into a [`Skill`], resolving `{{SKILL_DIR}}` to `skill_dir`.
///
/// Frontmatter is delimited by `---` lines. Only `name:` and `description:`
/// are read; everything else is ignored. The body is everything after the
/// closing `---`.
pub fn parse_skill_md(raw: &str, skill_dir: Option<&Path>) -> Result<Skill> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        bail!("SKILL.md is missing the opening `---` frontmatter delimiter");
    }

    let after_open = trimmed.find('\n').map(|i| &trimmed[i + 1..]).unwrap_or("");
    let close = after_open
        .find("\n---")
        .ok_or_else(|| eyre!("SKILL.md is missing the closing `---`"))?;

    let frontmatter = &after_open[..close];
    let body_raw = after_open[close..].trim_start_matches("\n---").trim_start();

    let mut name = String::new();
    let mut description = String::new();
    for line in frontmatter.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().trim_matches('"').trim_matches('\'').to_string();
        } else if let Some(v) = line.strip_prefix("description:") {
            description = v.trim().trim_matches('"').trim_matches('\'').to_string();
        }
    }

    if name.is_empty() {
        bail!("SKILL.md frontmatter is missing a `name:` field");
    }

    let content = if let Some(dir) = skill_dir {
        body_raw.replace("{{SKILL_DIR}}", &dir.to_string_lossy())
    } else {
        body_raw.to_string()
    };

    Ok(Skill {
        name,
        description,
        content,
    })
}

/// Look up a skill by name in `~/.axon/skills/<name>/SKILL.md`.
pub fn find_skill(name: &str) -> Result<Skill> {
    let skill_dir = skills_dir()?.join(name);
    let path = skill_dir.join("SKILL.md");
    let raw = fs::read_to_string(&path)
        .wrap_err_with(|| format!("skill '{}' not found in ~/.axon/skills/", name))?;
    parse_skill_md(&raw, Some(&skill_dir))
        .wrap_err_with(|| format!("failed to parse skill '{}'", name))
}

// ---------------------------------------------------------------------------
// GitHub download
// ---------------------------------------------------------------------------

struct GitHubRef {
    owner: String,
    repo: String,
    /// Branch or `HEAD` for bare repo URLs.
    branch: String,
    /// Path prefix inside the repo (empty = repo root).
    subpath: String,
}

fn parse_github_url(url: &str) -> Result<GitHubRef> {
    let url = url.trim_end_matches('/');
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .ok_or_else(|| eyre!("only github.com URLs are supported (got: {})", url))?;

    // Split into at most 5 parts: owner / repo / kind / branch / subpath
    let parts: Vec<&str> = path.splitn(5, '/').collect();

    let (owner, repo) = match parts.as_slice() {
        [o, r, ..] => (o.to_string(), r.to_string()),
        _ => bail!("GitHub URL must be at least https://github.com/owner/repo"),
    };

    let (branch, subpath) = match parts.as_slice() {
        // https://github.com/owner/repo
        [_, _] => ("HEAD".to_string(), String::new()),
        // https://github.com/owner/repo/blob/BRANCH/path/to/SKILL.md
        [_, _, "blob", branch, rest] => {
            // strip trailing /SKILL.md if present
            let sp = rest
                .strip_suffix("/SKILL.md")
                .unwrap_or_else(|| rest.strip_suffix("SKILL.md").unwrap_or(rest));
            (branch.to_string(), sp.trim_end_matches('/').to_string())
        }
        // https://github.com/owner/repo/tree/BRANCH/subpath
        [_, _, "tree", branch, subpath] => (
            branch.to_string(),
            subpath.trim_end_matches('/').to_string(),
        ),
        // https://github.com/owner/repo/tree/BRANCH
        [_, _, "tree", branch] => (branch.to_string(), String::new()),
        _ => bail!("unsupported GitHub URL format: {}", url),
    };

    Ok(GitHubRef {
        owner,
        repo,
        branch,
        subpath,
    })
}

fn fetch_text(url: &str) -> Result<String> {
    ureq::get(url)
        .set("User-Agent", "axon-cli")
        .call()
        .wrap_err_with(|| format!("failed to fetch {url}"))?
        .into_string()
        .wrap_err("failed to read response body")
}

/// Download a skill from a GitHub URL, save all supporting files to
/// `~/.axon/skills/<name>/`, and return the parsed [`Skill`].
pub fn download_and_save(url: &str) -> Result<Skill> {
    let gh = parse_github_url(url)?;

    // Download SKILL.md first to get the skill name.
    let skill_md_repo_path = if gh.subpath.is_empty() {
        "SKILL.md".to_string()
    } else {
        format!("{}/SKILL.md", gh.subpath)
    };
    let skill_md_raw_url = format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        gh.owner, gh.repo, gh.branch, skill_md_repo_path
    );
    eprintln!("Downloading SKILL.md...");
    let skill_md_content = fetch_text(&skill_md_raw_url)?;

    // Parse frontmatter (no SKILL_DIR yet — we need the name first).
    let skill_name = {
        let s = parse_skill_md(&skill_md_content, None)?;
        s.name
    };

    let dest_dir = skills_dir()?.join(&skill_name);
    fs::create_dir_all(&dest_dir)
        .wrap_err_with(|| format!("failed to create ~/.axon/skills/{}/", skill_name))?;

    // Save SKILL.md.
    fs::write(dest_dir.join("SKILL.md"), &skill_md_content).wrap_err("failed to save SKILL.md")?;

    // Fetch the full repo file tree via GitHub API.
    let tree_url = format!(
        "https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1",
        gh.owner, gh.repo, gh.branch
    );
    eprintln!("Fetching file tree...");
    let tree_json: serde_json::Value = serde_json::from_str(&fetch_text(&tree_url)?)
        .wrap_err("failed to parse GitHub tree API response")?;

    let entries = tree_json["tree"]
        .as_array()
        .ok_or_else(|| eyre!("unexpected GitHub Trees API response shape"))?;

    let prefix = if gh.subpath.is_empty() {
        String::new()
    } else {
        format!("{}/", gh.subpath)
    };

    let mut downloaded = 0usize;
    for entry in entries {
        if entry["type"].as_str() != Some("blob") {
            continue;
        }
        let repo_path = entry["path"].as_str().unwrap_or("");

        // For subpath repos, only include files under that subpath.
        let rel_path = if prefix.is_empty() {
            repo_path.to_string()
        } else if let Some(stripped) = repo_path.strip_prefix(&prefix) {
            stripped.to_string()
        } else {
            continue;
        };

        // Skip SKILL.md (already saved).
        if rel_path == "SKILL.md" {
            continue;
        }

        let raw_url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            gh.owner, gh.repo, gh.branch, repo_path
        );
        eprintln!("  {rel_path}");
        let content = fetch_text(&raw_url)?;

        let dest_file = dest_dir.join(&rel_path);
        if let Some(parent) = dest_file.parent() {
            fs::create_dir_all(parent)
                .wrap_err_with(|| format!("failed to create directory for {rel_path}"))?;
        }
        fs::write(&dest_file, &content).wrap_err_with(|| format!("failed to save {rel_path}"))?;
        downloaded += 1;
    }

    eprintln!(
        "Saved skill '{}' to ~/.axon/skills/{}/ ({} supporting files)",
        skill_name, skill_name, downloaded
    );

    // Re-parse with SKILL_DIR resolved.
    parse_skill_md(&skill_md_content, Some(&dest_dir))
        .wrap_err("failed to parse downloaded SKILL.md")
}

/// Resolve a skill from either a name (local lookup) or a GitHub URL
/// (downloads all files on first use, then caches locally).
pub fn resolve_skill(name_or_url: &str) -> Result<Skill> {
    if name_or_url.starts_with("http://") || name_or_url.starts_with("https://") {
        download_and_save(name_or_url)
    } else {
        find_skill(name_or_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_skill() {
        let raw =
            "---\nname: test-skill\ndescription: A test skill\n---\n\n# Instructions\nDo stuff.\n";
        let skill = parse_skill_md(raw, None).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill");
        assert!(skill.content.contains("Do stuff."));
    }

    #[test]
    fn parse_quoted_description() {
        let raw = "---\nname: my-skill\ndescription: \"Quoted description\"\n---\nBody.";
        let skill = parse_skill_md(raw, None).unwrap();
        assert_eq!(skill.description, "Quoted description");
    }

    #[test]
    fn parse_missing_name_errors() {
        let raw = "---\ndescription: No name here\n---\nBody.";
        assert!(parse_skill_md(raw, None).is_err());
    }

    #[test]
    fn skill_dir_placeholder_resolved() {
        let raw = "---\nname: s\ndescription: d\n---\nrun {{SKILL_DIR}}/setup.sh";
        let dir = Path::new("/home/user/.axon/skills/s");
        let skill = parse_skill_md(raw, Some(dir)).unwrap();
        assert_eq!(skill.content, "run /home/user/.axon/skills/s/setup.sh");
    }

    #[test]
    fn parse_github_url_bare_repo() {
        let gh = parse_github_url("https://github.com/org/my-skill").unwrap();
        assert_eq!(gh.owner, "org");
        assert_eq!(gh.repo, "my-skill");
        assert_eq!(gh.branch, "HEAD");
        assert_eq!(gh.subpath, "");
    }

    #[test]
    fn parse_github_url_blob() {
        let gh =
            parse_github_url("https://github.com/org/repo/blob/main/skills/code-review/SKILL.md")
                .unwrap();
        assert_eq!(gh.branch, "main");
        assert_eq!(gh.subpath, "skills/code-review");
    }

    #[test]
    fn parse_github_url_tree() {
        let gh =
            parse_github_url("https://github.com/org/repo/tree/main/skills/code-review").unwrap();
        assert_eq!(gh.branch, "main");
        assert_eq!(gh.subpath, "skills/code-review");
    }
}
