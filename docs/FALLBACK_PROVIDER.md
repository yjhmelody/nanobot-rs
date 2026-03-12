# LLM Provider Fallback Strategy

The fallback provider feature allows you to configure multiple LLM providers that will be tried in sequence when network issues or other retryable errors occur. This improves reliability by automatically switching to backup providers when the primary provider fails.

## How It Works

When a request fails with a retryable error (network issues, timeouts, rate limits), the system automatically tries the next provider in the configured list. Non-retryable errors (authentication failures, invalid configuration) immediately abort the fallback chain.

### Retryable Errors

The following errors trigger fallback to the next provider:
- Network connection failures
- Request timeouts
- Rate limit errors (429)
- Server errors (5xx)

### Non-Retryable Errors

The following errors stop the fallback chain immediately:
- Authentication failures (401, 403)
- Invalid configuration
- Model not available (404)
- Invalid API responses

## Configuration

Add the `fallbackProviders` field to your configuration file to enable fallback:

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
      "apiKey": "sk-ant-..."
    },
    "openai": {
      "apiKey": "sk-..."
    },
    "custom": {
      "apiKey": "...",
      "apiBase": "https://api.example.com/v1"
    }
  }
}
```

In this example:
1. Primary provider: `anthropic` (from `provider` field)
2. First fallback: `openai`
3. Second fallback: `custom`

## Behavior

### Successful Primary Provider

If the primary provider succeeds, no fallback occurs:

```
Request → Anthropic → Success ✓
```

### Primary Fails with Retryable Error

If the primary provider fails with a retryable error (e.g., timeout), the system tries the first fallback:

```
Request → Anthropic (timeout) → OpenAI → Success ✓
```

### Multiple Fallbacks

If multiple providers fail with retryable errors, all configured providers are tried:

```
Request → Anthropic (timeout) → OpenAI (rate limit) → Custom → Success ✓
```

### Non-Retryable Error

If any provider fails with a non-retryable error (e.g., authentication), the fallback chain stops immediately:

```
Request → Anthropic (auth error) → Error ✗
(OpenAI and Custom are not tried)
```

### All Providers Fail

If all providers fail with retryable errors, the last error is returned:

```
Request → Anthropic (timeout) → OpenAI (rate limit) → Custom (timeout) → Error ✗
(Returns the Custom provider's timeout error)
```

## Logging

The fallback provider logs detailed information about provider attempts:

```
DEBUG Attempting provider (provider_index=0, total_providers=3)
WARN  Provider failed with retryable error, trying next provider (provider_index=0, error="Request timeout after 30s")
DEBUG Attempting provider (provider_index=1, total_providers=3)
DEBUG Fallback provider succeeded (provider_index=1)
```

## Use Cases

### High Availability

Configure multiple providers to ensure your application continues working even when one provider has an outage:

```json
{
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4-5",
      "fallbackProviders": ["openai"]
    }
  }
}
```

### Rate Limit Mitigation

Distribute load across multiple providers to avoid hitting rate limits:

```json
{
  "agents": {
    "defaults": {
      "model": "openai/gpt-4",
      "fallbackProviders": ["anthropic", "custom"]
    }
  }
}
```

### Cost Optimization

Use a cheaper provider as fallback when the primary provider is unavailable:

```json
{
  "agents": {
    "defaults": {
      "model": "anthropic/claude-opus-4",
      "fallbackProviders": ["openai/gpt-4o-mini"]
    }
  }
}
```

## Best Practices

1. **Configure API Keys for All Providers**: Ensure all providers in the fallback chain have valid API keys configured.

2. **Test Your Configuration**: Verify that all providers work correctly before deploying to production.

3. **Monitor Fallback Usage**: Check logs to see how often fallbacks are triggered. Frequent fallbacks may indicate issues with your primary provider.

4. **Consider Model Compatibility**: Different providers may have different capabilities. Ensure your fallback providers support the features you need (e.g., tool calling, streaming).

5. **Set Appropriate Timeouts**: Configure reasonable timeout values to avoid waiting too long before trying the next provider.

## Limitations

- **Model Differences**: Different providers may produce different responses for the same prompt. The fallback provider uses the same model name, but behavior may vary.

- **No Automatic Retry**: The system tries each provider once. If all providers fail, the request fails. Consider implementing retry logic at a higher level if needed.

- **Streaming**: Fallback works for both streaming and non-streaming requests, but once a stream starts, it cannot fall back to another provider.

## Example Configuration

Complete example with fallback configuration:

```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nanobot/workspace",
      "model": "anthropic/claude-sonnet-4-5",
      "provider": "anthropic",
      "fallbackProviders": ["openai", "custom"],
      "maxTokens": 8192,
      "temperature": 0.1,
      "maxToolIterations": 40
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

## Implementation Details

The fallback provider is implemented in `src/provider/fallback.rs` and automatically wraps configured providers when `fallbackProviders` is set in the configuration. The `make_provider()` function in `src/provider/mod.rs` handles the creation of the fallback provider chain.
