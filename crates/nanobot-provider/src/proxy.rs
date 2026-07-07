//! Proxy fallback helper for LLM provider HTTP requests.
//!
//! This module provides [`ProxyFallbackHelper`], which implements a common pattern for
//! environments where a system proxy may be unreliable:
//!
//! 1. Send the request through the normally configured proxy (respecting env vars
//!    like `HTTP_PROXY`, `HTTPS_PROXY`, etc.).
//! 2. If the request fails with a gateway error (502, 503, 504) or a connection error,
//!    retry the request directly (bypassing the proxy).
//!
//! # When It Activates
//!
//! The helper only enables fallback logic when any standard proxy environment variable
//! is detected (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and their lowercase variants).
//! If no proxy is configured, all requests go through the primary client with no retries.

use std::env;

use reqwest::StatusCode;
use tracing::warn;

/// Logging target for provider-related trace messages.
pub const TARGET: &str = "nanobot::provider";

/// Helper for implementing proxy fallback logic in LLM providers.
///
/// This struct manages the pattern of:
/// 1. Try request with system proxy settings
/// 2. If it fails with gateway errors (502/503/504), retry without proxy
/// 3. If primary request fails entirely, retry without proxy
#[derive(Debug)]
pub struct ProxyFallbackHelper {
    client: reqwest::Client,
    direct_client: reqwest::Client,
    enabled: bool,
}

impl ProxyFallbackHelper {
    /// Create a new proxy fallback helper.
    ///
    /// The helper will only be enabled if proxy environment variables are detected.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let direct_client = reqwest::Client::builder()
            .use_rustls_tls()
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            direct_client,
            enabled: has_proxy_env_configured(),
        }
    }

    /// Get the primary client (respects system proxy settings).
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Get the direct client (bypasses proxy).
    pub fn direct_client(&self) -> &reqwest::Client {
        &self.direct_client
    }

    /// Check if proxy fallback is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Handle a response with potential proxy fallback.
    ///
    /// If the response has a gateway error status (502/503/504) and proxy fallback is enabled,
    /// this will log a warning and return `true` to indicate a retry should be attempted.
    pub fn should_retry_response(&self, status: StatusCode, endpoint: &str) -> bool {
        if self.enabled && should_retry_without_proxy_status(status) {
            warn!(
                target: TARGET,
                status = %status,
                endpoint = %endpoint,
                "Gateway error with proxy, retrying without proxy"
            );
            true
        } else {
            false
        }
    }

    /// Log that a direct retry is being attempted after a request error.
    pub fn log_retry_after_error(&self, endpoint: &str, error: &impl std::fmt::Display) {
        warn!(
            target: TARGET,
            endpoint,
            error = %error,
            "primary provider request failed, retrying without proxy"
        );
    }

    /// Log that a direct retry failed.
    pub fn log_retry_failed(&self, endpoint: &str, error: &impl std::fmt::Display) {
        warn!(
            target: TARGET,
            endpoint,
            error = %error,
            "direct provider retry failed after gateway error"
        );
    }

    /// Handle a request error with potential proxy fallback.
    ///
    /// If the request failed entirely and proxy fallback is enabled,
    /// this will log a warning and return `true` to indicate a retry should be attempted.
    pub fn should_retry_error(&self, error: &reqwest::Error, endpoint: &str) -> bool {
        if self.enabled && (error.is_connect() || error.is_timeout()) {
            warn!(
                target: TARGET,
                error = %error,
                endpoint = %endpoint,
                "Connection error with proxy, retrying without proxy"
            );
            true
        } else {
            false
        }
    }
}

impl Default for ProxyFallbackHelper {
    fn default() -> Self {
        Self::new()
    }
}

/// Checks whether any standard proxy environment variable is set (upper and lower case).
///
/// Checks: `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `http_proxy`, `https_proxy`, `all_proxy`.
fn has_proxy_env_configured() -> bool {
    [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ]
    .into_iter()
    .any(|key| env::var_os(key).is_some())
}

/// Returns `true` if the HTTP status code is a gateway error (502, 503, 504)
/// that might be caused by a misconfigured or broken proxy.
fn should_retry_without_proxy_status(status: StatusCode) -> bool {
    (502..=504).contains(&status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_without_proxy_status_covers_gateway_failures() {
        assert!(should_retry_without_proxy_status(StatusCode::BAD_GATEWAY));
        assert!(should_retry_without_proxy_status(
            StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(should_retry_without_proxy_status(
            StatusCode::GATEWAY_TIMEOUT
        ));
        assert!(!should_retry_without_proxy_status(StatusCode::BAD_REQUEST));
    }
}
