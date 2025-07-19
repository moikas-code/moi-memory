#[cfg(test)]
mod timeout_integration_tests {
    use super::super::timeout_manager::{TimeoutManager, OperationType};
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_timeout_manager_integration() {
        let manager = TimeoutManager::with_default_config();
        
        // Test successful operation
        let result = manager.execute_with_timeout(
            OperationType::Query,
            "Test successful operation".to_string(),
            None,
            async { 
                sleep(Duration::from_millis(10)).await;
                Ok::<String, String>("Success".to_string()) 
            }
        ).await;
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success");
        
        // Verify stats
        let stats = manager.get_stats().await;
        assert_eq!(stats.completed_operations, 1);
        assert_eq!(stats.timed_out_operations, 0);
        
        println!("✅ Timeout manager integration test passed");
    }
    
    #[tokio::test]
    async fn test_timeout_occurs() {
        let manager = TimeoutManager::with_default_config();
        
        // Test operation that times out
        let result = manager.execute_with_custom_timeout(
            OperationType::Query,
            "Test timeout operation".to_string(),
            None,
            Duration::from_millis(50),
            async { 
                sleep(Duration::from_millis(100)).await;
                Ok::<String, String>("Should not complete".to_string()) 
            }
        ).await;
        
        assert!(result.is_err());
        assert!(result.unwrap_err().is_timeout());
        
        // Verify stats
        let stats = manager.get_stats().await;
        assert_eq!(stats.timed_out_operations, 1);
        
        println!("✅ Timeout behavior test passed");
    }
}