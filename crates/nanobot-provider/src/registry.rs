//! Provider-specific model name specification registry.
//!
//! This module maintains a small registry of [`ProviderSpec`] entries that define
//! how model names should be canonically resolved for each provider. The registry
//! is used by [`OpenAICompatProvider`](crate::openai_compat::OpenAICompatProvider)
//! to ensure that model names sent to each provider match what that provider expects.
//!
//! # Current Entries
//!
//! | Provider          | litellm_prefix     | Notes                          |
//! |-------------------|--------------------|--------------------------------|
//! | `github_copilot`  | `github_copilot`   | Already prefixed, no stripping |

/// Specification for how a provider handles model name resolution.
///
/// Each entry describes the canonicalization rules for a specific provider name.
#[derive(Debug, Clone)]
pub struct ProviderSpec {
    /// Provider name as it appears in configuration.
    pub name: &'static str,
    /// The litellm-style prefix that should be prepended to model names when
    /// communicating with this provider (e.g., "github_copilot").
    pub litellm_prefix: &'static str,
    /// List of prefixes that, when matched, indicate the model name is already
    /// correctly prefixed and should not be modified further.
    pub skip_prefixes: &'static [&'static str],
    /// If `true`, strip the provider name prefix from the model name before
    /// canonicalizing (e.g., `"openai/gpt-4"` → `"gpt-4"`).
    pub strip_model_prefix: bool,
}

/// Looks up a [`ProviderSpec`] by provider name.
///
/// Returns `None` if no spec is registered for the given name.
pub fn find_spec(name: &str) -> Option<ProviderSpec> {
    specs().into_iter().find(|s| s.name == name)
}

/// Returns the full list of registered provider specs.
///
/// This is a static vec; new entries should be added here when supporting
/// new providers that require non-standard model name resolution.
fn specs() -> Vec<ProviderSpec> {
    vec![ProviderSpec {
        name: "github_copilot",
        litellm_prefix: "github_copilot",
        skip_prefixes: &["github_copilot/"],
        strip_model_prefix: false,
    }]
}
