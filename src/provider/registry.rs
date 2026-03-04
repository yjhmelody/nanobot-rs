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
    vec![
        ProviderSpec {
            name: "openrouter",
            litellm_prefix: "openrouter",
            skip_prefixes: &[],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "aihubmix",
            litellm_prefix: "openai",
            skip_prefixes: &[],
            strip_model_prefix: true,
        },
        ProviderSpec {
            name: "siliconflow",
            litellm_prefix: "openai",
            skip_prefixes: &[],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "volcengine",
            litellm_prefix: "volcengine",
            skip_prefixes: &[],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "deepseek",
            litellm_prefix: "deepseek",
            skip_prefixes: &["deepseek/"],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "gemini",
            litellm_prefix: "gemini",
            skip_prefixes: &["gemini/"],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "dashscope",
            litellm_prefix: "dashscope",
            skip_prefixes: &["dashscope/", "openrouter/"],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "moonshot",
            litellm_prefix: "moonshot",
            skip_prefixes: &["moonshot/", "openrouter/"],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "minimax",
            litellm_prefix: "minimax",
            skip_prefixes: &["minimax/", "openrouter/"],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "zhipu",
            litellm_prefix: "zai",
            skip_prefixes: &["zhipu/", "zai/", "openrouter/", "hosted_vllm/"],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "vllm",
            litellm_prefix: "hosted_vllm",
            skip_prefixes: &[],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "groq",
            litellm_prefix: "groq",
            skip_prefixes: &["groq/"],
            strip_model_prefix: false,
        },
        ProviderSpec {
            name: "github_copilot",
            litellm_prefix: "github_copilot",
            skip_prefixes: &["github_copilot/"],
            strip_model_prefix: false,
        },
    ]
}
