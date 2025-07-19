use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use anyhow::{Result, anyhow};
use tracing::{info, warn, error};

/// Circuit breaker states
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitState {
    /// Circuit is closed - requests pass through normally
    Closed,
    /// Circuit is open - requests fail immediately
    Open,
    /// Circuit is half-open - limited requests allowed to test recovery
    HalfOpen,
}

/// Circuit breaker configuration
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures before opening the circuit
    pub failure_threshold: u32,
    /// Success threshold to close the circuit from half-open
    pub success_threshold: u32,
    /// Duration to wait before attempting recovery
    pub timeout: Duration,
    /// Time window for counting failures
    pub window: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            timeout: Duration::from_secs(30),
            window: Duration::from_secs(60),
        }
    }
}

/// Circuit breaker for managing connection failures
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Arc<RwLock<CircuitState>>,
    failure_count: Arc<RwLock<u32>>,
    success_count: Arc<RwLock<u32>>,
    last_failure_time: Arc<RwLock<Option<Instant>>>,
    last_state_change: Arc<RwLock<Instant>>,
    window_start: Arc<RwLock<Instant>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            failure_count: Arc::new(RwLock::new(0)),
            success_count: Arc::new(RwLock::new(0)),
            last_failure_time: Arc::new(RwLock::new(None)),
            last_state_change: Arc::new(RwLock::new(Instant::now())),
            window_start: Arc::new(RwLock::new(Instant::now())),
        }
    }
    
    /// Check if request should be allowed
    pub async fn can_proceed(&self) -> Result<()> {
        let mut state = self.state.write().await;
        
        match *state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                // Check if timeout has passed
                let last_change = *self.last_state_change.read().await;
                if last_change.elapsed() >= self.config.timeout {
                    // Transition to half-open
                    *state = CircuitState::HalfOpen;
                    *self.last_state_change.write().await = Instant::now();
                    *self.success_count.write().await = 0;
                    info!("Circuit breaker transitioning to half-open state");
                    Ok(())
                } else {
                    Err(anyhow!("Circuit breaker is open"))
                }
            }
            CircuitState::HalfOpen => {
                // Allow limited requests in half-open state
                Ok(())
            }
        }
    }
    
    /// Record a successful operation
    pub async fn record_success(&self) {
        let state = *self.state.read().await;
        
        match state {
            CircuitState::Closed => {
                // Reset failure count in time window
                self.reset_window_if_needed().await;
            }
            CircuitState::HalfOpen => {
                let mut success_count = self.success_count.write().await;
                *success_count += 1;
                
                if *success_count >= self.config.success_threshold {
                    // Close the circuit
                    *self.state.write().await = CircuitState::Closed;
                    *self.last_state_change.write().await = Instant::now();
                    *self.failure_count.write().await = 0;
                    info!("Circuit breaker closed after successful recovery");
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but handle gracefully
                warn!("Success recorded while circuit is open");
            }
        }
    }
    
    /// Record a failed operation
    pub async fn record_failure(&self) {
        let state = *self.state.read().await;
        
        match state {
            CircuitState::Closed => {
                self.reset_window_if_needed().await;
                
                let mut failure_count = self.failure_count.write().await;
                *failure_count += 1;
                *self.last_failure_time.write().await = Some(Instant::now());
                
                if *failure_count >= self.config.failure_threshold {
                    // Open the circuit
                    *self.state.write().await = CircuitState::Open;
                    *self.last_state_change.write().await = Instant::now();
                    error!("Circuit breaker opened after {} failures", *failure_count);
                }
            }
            CircuitState::HalfOpen => {
                // Failure in half-open state reopens the circuit
                *self.state.write().await = CircuitState::Open;
                *self.last_state_change.write().await = Instant::now();
                *self.failure_count.write().await = 0;
                warn!("Circuit breaker reopened after failure in half-open state");
            }
            CircuitState::Open => {
                // Already open, update last failure time
                *self.last_failure_time.write().await = Some(Instant::now());
            }
        }
    }
    
    /// Get current circuit state
    pub async fn get_state(&self) -> CircuitState {
        *self.state.read().await
    }
    
    /// Get circuit breaker statistics
    pub async fn get_stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            state: *self.state.read().await,
            failure_count: *self.failure_count.read().await,
            success_count: *self.success_count.read().await,
            last_failure_time: *self.last_failure_time.read().await,
            last_state_change: *self.last_state_change.read().await,
        }
    }
    
    /// Reset the failure count window if needed
    async fn reset_window_if_needed(&self) {
        let mut window_start = self.window_start.write().await;
        if window_start.elapsed() >= self.config.window {
            *window_start = Instant::now();
            *self.failure_count.write().await = 0;
        }
    }
    
    /// Execute a function with circuit breaker protection
    pub async fn call<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        // Check if we can proceed
        self.can_proceed().await?;
        
        // Execute the function
        match f() {
            Ok(result) => {
                self.record_success().await;
                Ok(result)
            }
            Err(e) => {
                self.record_failure().await;
                Err(e)
            }
        }
    }
    
    /// Execute an async function with circuit breaker protection
    pub async fn call_async<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        // Check if we can proceed
        self.can_proceed().await?;
        
        // Execute the async function
        match f().await {
            Ok(result) => {
                self.record_success().await;
                Ok(result)
            }
            Err(e) => {
                self.record_failure().await;
                Err(e)
            }
        }
    }
}

/// Circuit breaker statistics
#[derive(Debug)]
pub struct CircuitBreakerStats {
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    pub last_failure_time: Option<Instant>,
    pub last_state_change: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_circuit_breaker_opens_on_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);
        
        // Record failures
        for _ in 0..3 {
            cb.record_failure().await;
        }
        
        // Circuit should be open
        assert_eq!(cb.get_state().await, CircuitState::Open);
        
        // Requests should fail
        assert!(cb.can_proceed().await.is_err());
    }
    
    #[tokio::test]
    async fn test_circuit_breaker_half_open_transition() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(50),
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);
        
        // Open the circuit
        cb.record_failure().await;
        assert_eq!(cb.get_state().await, CircuitState::Open);
        
        // Wait for timeout
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Should transition to half-open
        assert!(cb.can_proceed().await.is_ok());
        assert_eq!(cb.get_state().await, CircuitState::HalfOpen);
    }
    
    #[tokio::test]
    async fn test_circuit_breaker_closes_on_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            timeout: Duration::from_millis(50),
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);
        
        // Open the circuit
        cb.record_failure().await;
        
        // Wait and transition to half-open
        tokio::time::sleep(Duration::from_millis(100)).await;
        cb.can_proceed().await.unwrap();
        
        // Record successes
        cb.record_success().await;
        cb.record_success().await;
        
        // Circuit should be closed
        assert_eq!(cb.get_state().await, CircuitState::Closed);
    }
}