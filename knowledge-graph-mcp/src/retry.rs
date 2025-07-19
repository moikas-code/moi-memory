use anyhow::Result;
use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{warn, debug};

/// Configuration for retry behavior
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Factor to multiply delay by after each retry
    pub backoff_factor: f64,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Jitter to add randomness to delays (0.0 to 1.0)
    pub jitter: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            backoff_factor: 2.0,
            max_delay: Duration::from_secs(30),
            jitter: 0.1,
        }
    }
}

/// Retry a fallible async operation with exponential backoff
pub async fn retry_with_backoff<F, Fut, T, E>(
    operation: F,
    config: RetryConfig,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut delay = config.initial_delay;
    
    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => {
                if attempt > 0 {
                    debug!("Operation succeeded after {} retries", attempt);
                }
                return Ok(result);
            }
            Err(e) if attempt < config.max_retries => {
                warn!(
                    "Attempt {}/{} failed: {}, retrying in {:?}",
                    attempt + 1,
                    config.max_retries,
                    e,
                    delay
                );
                
                // Add jitter to prevent thundering herd
                let jitter_amount = if config.jitter > 0.0 {
                    let jitter_ms = (delay.as_millis() as f64 * config.jitter) as u64;
                    Duration::from_millis(rand::random::<u64>() % jitter_ms.max(1))
                } else {
                    Duration::ZERO
                };
                
                sleep(delay + jitter_amount).await;
                
                // Calculate next delay with exponential backoff
                delay = Duration::from_secs_f64(
                    (delay.as_secs_f64() * config.backoff_factor).min(config.max_delay.as_secs_f64())
                );
            }
            Err(e) => {
                warn!("All {} retry attempts failed", config.max_retries);
                return Err(e);
            }
        }
    }
    
    unreachable!("Retry loop should have returned by now")
}

/// Retry configuration presets
pub mod presets {
    use super::*;
    
    /// Fast retry for operations that should fail quickly
    pub fn fast() -> RetryConfig {
        RetryConfig {
            max_retries: 3,
            initial_delay: Duration::from_millis(50),
            backoff_factor: 2.0,
            max_delay: Duration::from_secs(1),
            jitter: 0.1,
        }
    }
    
    /// Standard retry for most operations
    pub fn standard() -> RetryConfig {
        RetryConfig::default()
    }
    
    /// Aggressive retry for critical operations
    pub fn aggressive() -> RetryConfig {
        RetryConfig {
            max_retries: 5,
            initial_delay: Duration::from_millis(500),
            backoff_factor: 2.0,
            max_delay: Duration::from_secs(60),
            jitter: 0.2,
        }
    }
    
    /// Patient retry for operations that may take time to recover
    pub fn patient() -> RetryConfig {
        RetryConfig {
            max_retries: 10,
            initial_delay: Duration::from_secs(1),
            backoff_factor: 1.5,
            max_delay: Duration::from_secs(300),
            jitter: 0.3,
        }
    }
}

/// Helper function to determine if an error is retryable
pub fn is_retryable_error<E: std::fmt::Display>(error: &E) -> bool {
    let error_string = error.to_string().to_lowercase();
    
    // Network and connection errors
    error_string.contains("connection") ||
    error_string.contains("timeout") ||
    error_string.contains("refused") ||
    error_string.contains("reset") ||
    error_string.contains("broken pipe") ||
    
    // Temporary failures
    error_string.contains("temporarily") ||
    error_string.contains("unavailable") ||
    error_string.contains("too many requests") ||
    error_string.contains("rate limit") ||
    
    // Database errors
    error_string.contains("deadlock") ||
    error_string.contains("lock") ||
    error_string.contains("busy") ||
    
    // Service errors
    error_string.contains("503") ||
    error_string.contains("502") ||
    error_string.contains("504")
}

/// Macro for easy retry usage
#[macro_export]
macro_rules! retry {
    ($operation:expr) => {
        retry_with_backoff(
            || async { $operation },
            RetryConfig::default()
        ).await
    };
    ($operation:expr, $config:expr) => {
        retry_with_backoff(
            || async { $operation },
            $config
        ).await
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    
    #[tokio::test]
    async fn test_retry_success_first_attempt() {
        let result = retry_with_backoff(
            || async { Ok::<_, anyhow::Error>(42) },
            RetryConfig::default()
        ).await;
        
        assert_eq!(result.unwrap(), 42);
    }
    
    #[tokio::test]
    async fn test_retry_success_after_failures() {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_count_clone = attempt_count.clone();
        
        let result = retry_with_backoff(
            || {
                let count = attempt_count_clone.clone();
                async move {
                    let attempts = count.fetch_add(1, Ordering::SeqCst);
                    if attempts < 2 {
                        Err(anyhow::anyhow!("Temporary failure"))
                    } else {
                        Ok(42)
                    }
                }
            },
            presets::fast()
        ).await;
        
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    }
    
    #[tokio::test]
    async fn test_retry_all_attempts_fail() {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_count_clone = attempt_count.clone();
        
        let result = retry_with_backoff(
            || {
                let count = attempt_count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err::<i32, _>(anyhow::anyhow!("Permanent failure"))
                }
            },
            presets::fast()
        ).await;
        
        assert!(result.is_err());
        assert_eq!(attempt_count.load(Ordering::SeqCst), 4); // initial + 3 retries
    }
    
    #[test]
    fn test_is_retryable_error() {
        assert!(is_retryable_error(&anyhow::anyhow!("Connection refused")));
        assert!(is_retryable_error(&anyhow::anyhow!("Operation timed out")));
        assert!(is_retryable_error(&anyhow::anyhow!("Service temporarily unavailable")));
        assert!(is_retryable_error(&anyhow::anyhow!("Too many requests")));
        
        assert!(!is_retryable_error(&anyhow::anyhow!("Invalid configuration")));
        assert!(!is_retryable_error(&anyhow::anyhow!("File not found")));
    }
}