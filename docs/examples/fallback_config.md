# Example Configuration with Fallback Providers

This example demonstrates how to configure multiple LLM providers with automatic fallback.

## Basic Fallback Configuration

```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nanobot/workspace",
      "model": "anthropic/claude-sonnet-4-5",
      "provider": "anthropic",
      "fallbackProviders": ["openai"],
      "maxTokens": 8192,
      "temperature": 0.1
    }
  },
  "providers": {
    "anthropic": {
      "apiKey": "${ANTHROPIC_API_KEY}"
    },
    "openai": {
      "apiKey": "${OPENAI_API_KEY}"
    }
  }
}
```

## Multi-Provider Fallback

```json
{
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4-5",
      "provider": "anthropic",
      "fallbackProviders": ["openai", "custom"]
    }
  },
  "providers": {
    "anthropic": {
      "apiKey": "${ANTHROPIC_API_KEY}"
    },
    "openai": {
      "apiKey": "${OPENAI_API_KEY}"
    },
    "custom": {
      "apiKey": "${CUSTOM_API_KEY}",
      "apiBase": "https://api.example.com/v1"
    }
  }
}
```

## Testing the Configuration

1. Set environment variables:
```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
```

2. Start the agent:
```bash
cargo run -- agent
```

3. Test fallback by temporarily disabling the primary provider (e.g., invalid API key) and observe the logs showing fallback to the next provider.

## Expected Behavior

### Normal Operation
```
[DEBUG] Attempting provider (provider_index=0, total_providers=2)
[INFO]  Request completed successfully
```

### Fallback Triggered
```
[DEBUG] Attempting provider (provider_index=0, total_providers=2)
[WARN]  Provider failed with retryable error, trying next provider (error="Request timeout after 30s")
[DEBUG] Attempting provider (provider_index=1, total_providers=2)
[DEBUG] Fallback provider succeeded (provider_index=1)
[INFO]  Request completed successfully
```

### All Providers Failed
```
[DEBUG] Attempting provider (provider_index=0, total_providers=2)
[WARN]  Provider failed with retryable error, trying next provider (error="Request timeout after 30s")
[DEBUG] Attempting provider (provider_index=1, total_providers=2)
[WARN]  Provider failed with retryable error, trying next provider (error="Rate limit exceeded")
[ERROR] All providers failed
```
