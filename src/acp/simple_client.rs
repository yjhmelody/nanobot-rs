//! Simplified ACP Client implementation

use agent_client_protocol::{
    Client, SessionNotification, RequestPermissionRequest,
    RequestPermissionResponse, RequestPermissionOutcome, 
    SelectedPermissionOutcome, Result,
};

/// Simplified Client implementation for nanobot-rs
/// 
/// This provides a minimal implementation of the ACP Client trait,
/// automatically approving all permission requests and logging notifications.
pub struct SimpleClient;

#[async_trait::async_trait(?Send)]
impl Client for SimpleClient {
    /// Auto-approve all permission requests
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse> {
        // Auto-approve: select the first option if available
        let outcome = if let Some(first_option) = args.options.first() {
            // Use the new() constructor with option_id field
            RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(first_option.option_id.clone())
            )
        } else {
            RequestPermissionOutcome::Cancelled
        };
        
        Ok(RequestPermissionResponse::new(outcome))
    }
    
    /// Log session notifications
    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        // Simplified: just log (using eprintln since log might not be configured)
        eprintln!("Session notification: {:?}", args);
        Ok(())
    }
    
    // Other methods use default implementation (return method_not_found)
}
