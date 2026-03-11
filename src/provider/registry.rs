#[derive(Debug, Clone)]
pub struct ProviderSpec {
    pub name: &'static str,
    pub litellm_prefix: &'static str,
    pub skip_prefixes: &'static [&'static str],
    pub strip_model_prefix: bool,
}

pub fn find_spec(name: &str) -> Option<ProviderSpec> {
    specs().into_iter().find(|s| s.name == name)
}

fn specs() -> Vec<ProviderSpec> {
    vec![ProviderSpec {
        name: "github_copilot",
        litellm_prefix: "github_copilot",
        skip_prefixes: &["github_copilot/"],
        strip_model_prefix: false,
    }]
}
