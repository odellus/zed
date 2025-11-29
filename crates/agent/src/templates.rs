use anyhow::Result;
use collections::HashMap;
use gpui::SharedString;
use handlebars::Handlebars;
use parking_lot::RwLock;
use rust_embed::RustEmbed;
use serde::Serialize;
use settings::PromptTemplate;
use std::sync::Arc;

#[derive(RustEmbed)]
#[folder = "src/templates"]
#[include = "*.hbs"]
struct Assets;

/// Holds the handlebars templates and tracks prompt IDs for telemetry
pub struct Templates {
    handlebars: Handlebars<'static>,
    /// Maps template name -> (template_content, prompt_id)
    /// prompt_id is set after registration with the database
    prompt_registry: RwLock<HashMap<String, PromptInfo>>,
}

#[derive(Clone, Debug)]
pub struct PromptInfo {
    pub template_content: String,
    pub prompt_id: Option<String>,
}

impl Templates {
    pub fn new() -> Arc<Self> {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars.register_helper("contains", Box::new(contains));
        handlebars.register_embed_templates::<Assets>().unwrap();

        // Build the prompt registry from embedded assets
        let mut prompt_registry = HashMap::default();
        for path in Assets::iter() {
            if let Some(asset) = Assets::get(&path) {
                let template_name = path.to_string();
                let template_content = String::from_utf8_lossy(&asset.data).to_string();
                prompt_registry.insert(
                    template_name,
                    PromptInfo {
                        template_content,
                        prompt_id: None,
                    },
                );
            }
        }

        Arc::new(Self {
            handlebars,
            prompt_registry: RwLock::new(prompt_registry),
        })
    }

    /// Get all template names and their content (for registration with db)
    pub fn all_templates(&self) -> Vec<(String, String)> {
        self.prompt_registry
            .read()
            .iter()
            .map(|(name, info)| (name.clone(), info.template_content.clone()))
            .collect()
    }

    /// Set the prompt_id for a template after database registration
    pub fn set_prompt_id(&self, template_name: &str, prompt_id: String) {
        if let Some(info) = self.prompt_registry.write().get_mut(template_name) {
            info.prompt_id = Some(prompt_id);
        }
    }

    /// Get the prompt_id for a template (if registered)
    pub fn get_prompt_id(&self, template_name: &str) -> Option<String> {
        self.prompt_registry
            .read()
            .get(template_name)
            .and_then(|info| info.prompt_id.clone())
    }

    /// Get template content by name
    pub fn get_template_content(&self, template_name: &str) -> Option<String> {
        self.prompt_registry
            .read()
            .get(template_name)
            .map(|info| info.template_content.clone())
    }

    /// Render a template by name with the given data
    pub fn render_template<T: Serialize>(&self, template_name: &str, data: &T) -> Result<String> {
        Ok(self.handlebars.render(template_name, data)?)
    }

    /// Register all templates with the database and store their prompt IDs.
    /// This should be called once during initialization.
    pub async fn register_with_database(&self, db: &crate::ThreadsDatabase) -> Result<()> {
        let templates = self.all_templates();

        for (name, content) in templates {
            // Register returns existing ID if same hash, or creates new version
            let prompt_id = db.register_prompt(name.clone(), content, None).await?;
            self.set_prompt_id(&name, prompt_id);
        }

        log::info!(
            "Registered {} prompt templates with database",
            self.prompt_registry.read().len()
        );

        Ok(())
    }
}

pub trait Template: Sized {
    const TEMPLATE_NAME: &'static str;

    fn render(&self, templates: &Templates) -> Result<String>
    where
        Self: Serialize + Sized,
    {
        Ok(templates.handlebars.render(Self::TEMPLATE_NAME, self)?)
    }
}

#[derive(Serialize)]
pub struct SystemPromptTemplate<'a> {
    #[serde(flatten)]
    pub project: &'a prompt_store::ProjectContext,
    pub available_tools: Vec<SharedString>,
    pub model_name: Option<String>,
}

impl SystemPromptTemplate<'_> {
    /// Render using a specific prompt template variant.
    pub fn render_with_template(
        &self,
        templates: &Templates,
        prompt_template: PromptTemplate,
    ) -> Result<String> {
        let template_name = prompt_template.template_name();
        Ok(templates.handlebars.render(template_name, self)?)
    }
}

impl Template for SystemPromptTemplate<'_> {
    const TEMPLATE_NAME: &'static str = "system_prompt.hbs";
}

/// Handlebars helper for checking if an item is in a list
fn contains(
    h: &handlebars::Helper,
    _: &handlebars::Handlebars,
    _: &handlebars::Context,
    _: &mut handlebars::RenderContext,
    out: &mut dyn handlebars::Output,
) -> handlebars::HelperResult {
    let list = h
        .param(0)
        .and_then(|v| v.value().as_array())
        .ok_or_else(|| {
            handlebars::RenderError::new("contains: missing or invalid list parameter")
        })?;
    let query = h.param(1).map(|v| v.value()).ok_or_else(|| {
        handlebars::RenderError::new("contains: missing or invalid query parameter")
    })?;

    if list.contains(query) {
        out.write("true")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_prompt_template() {
        let project = prompt_store::ProjectContext::default();
        let template = SystemPromptTemplate {
            project: &project,
            available_tools: vec!["echo".into()],
            model_name: Some("test-model".to_string()),
        };
        let templates = Templates::new();
        let rendered = template.render(&templates).unwrap();
        assert!(rendered.contains("## Fixing Diagnostics"));
        assert!(rendered.contains("test-model"));
    }
}
