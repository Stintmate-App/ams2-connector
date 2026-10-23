//! The control app's texts: the embedded English catalog with the workspace's packs over it.

use ams2_i18n::{Arg, Catalog};
use std::path::Path;
use std::sync::Arc;

/// A resolved catalog, cheap to clone.
#[derive(Clone)]
pub struct Texts(Arc<Catalog>);

impl Texts {
    /// The embedded English texts.
    pub fn english() -> Self {
        Self(Arc::new(Catalog::english(ams2_i18n::embedded_messages())))
    }

    /// The language of `<workspace>\hub\settings.json` with the workspace's packs.
    pub fn load(workspace: Option<&Path>) -> Self {
        let language = ams2_i18n::read_language(workspace);
        let loaded = ams2_i18n::load(workspace, &language, ams2_i18n::embedded_messages());
        for (path, why) in &loaded.skipped {
            tracing::warn!("Language pack {} skipped: {why:?}", path.display());
        }
        Self(Arc::new(loaded.catalog))
    }

    /// The text of `key` with its placeholders filled.
    pub fn t(&self, key: &str, params: &[(&str, Arg)]) -> String {
        self.0.text(key, params)
    }

    /// The text of a key with no placeholders.
    pub fn s(&self, key: &str) -> String {
        self.0.text(key, &[])
    }

    /// The built-in English text of a plain message (for files that stay English).
    pub fn english_text(key: &str) -> String {
        ams2_i18n::embedded_messages()
            .get(key)
            .and_then(|m| m.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or(key)
            .to_string()
    }
}
