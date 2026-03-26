use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

/// Skill activation tool for compact mode.
///
/// When skills are in compact mode (name+description only in system prompt),
/// the model must call this tool to load the full skill instructions before
/// executing skill-related tasks.
///
/// This is the preferred alternative to `read_skill` — named to match the
/// Gemini CLI convention of `activate_skill`, which models are more likely
/// to call proactively.
pub struct ActivateSkillTool {
    workspace_dir: PathBuf,
    open_skills_enabled: bool,
    open_skills_dir: Option<String>,
}

impl ActivateSkillTool {
    pub fn new(
        workspace_dir: PathBuf,
        open_skills_enabled: bool,
        open_skills_dir: Option<String>,
    ) -> Self {
        Self {
            workspace_dir,
            open_skills_enabled,
            open_skills_dir,
        }
    }
}

#[async_trait]
impl Tool for ActivateSkillTool {
    fn name(&self) -> &str {
        "activate_skill"
    }

    fn description(&self) -> &str {
        "Activate a skill to load its full instructions. You MUST call this before performing any task that matches a skill's description. The skill's complete instructions, templates, and workflow will be returned."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "The exact skill name from <available_skills>. Example: 'cortex-reports'"
                }
            },
            "required": ["skill_name"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        // Accept both "skill_name" and "name" for compatibility
        let requested = args
            .get("skill_name")
            .or_else(|| args.get("name"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'skill_name' parameter"))?;

        let skills = crate::skills::load_skills_with_open_skills_settings(
            &self.workspace_dir,
            self.open_skills_enabled,
            self.open_skills_dir.as_deref(),
        );

        let Some(skill) = skills
            .iter()
            .find(|skill| skill.name.eq_ignore_ascii_case(requested))
        else {
            let mut names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
            names.sort_unstable();
            let available = if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            };

            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Unknown skill '{requested}'. Available skills: {available}"
                )),
            });
        };

        let Some(location) = skill.location.as_ref() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Skill '{}' has no readable source location.",
                    skill.name
                )),
            });
        };

        match tokio::fs::read_to_string(location).await {
            Ok(content) => {
                let output = format!(
                    "=== Skill '{}' Activated ===\n\
                     Follow these instructions to complete the task.\n\n\
                     {}",
                    skill.name, content
                );
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(err) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Failed to activate skill '{}' from {}: {err}",
                    skill.name,
                    location.display()
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_tool(tmp: &TempDir) -> ActivateSkillTool {
        ActivateSkillTool::new(tmp.path().join("workspace"), false, None)
    }

    #[tokio::test]
    async fn activates_skill_by_name() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("workspace/skills/reports");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: reports\ndescription: Generate reports\n---\n\n# Reports\n\nUse create-report.sh\n",
        )
        .unwrap();

        let result = make_tool(&tmp)
            .execute(json!({ "skill_name": "reports" }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Skill 'reports' Activated"));
        assert!(result.output.contains("create-report.sh"));
    }

    #[tokio::test]
    async fn accepts_name_param_for_compatibility() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("workspace/skills/email");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Email\n").unwrap();

        let result = make_tool(&tmp)
            .execute(json!({ "name": "email" }))
            .await
            .unwrap();

        assert!(result.success);
    }

    #[tokio::test]
    async fn unknown_skill_lists_available() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("workspace/skills/weather");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Weather\n").unwrap();

        let result = make_tool(&tmp)
            .execute(json!({ "skill_name": "unknown" }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("weather"));
    }
}
