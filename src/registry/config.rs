use std::time::Duration;
use serde::{Serialize, Deserialize};

/// Configuration for the adapter registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    /// How often to run health checks
    pub health_check_interval: Duration,
    
    /// How long to wait before disabling unhealthy adapters
    pub max_unhealthy_duration: Duration,
    
    /// Maximum number of adapters per chain
    pub max_adapters_per_chain: usize,
    
    /// Whether to allow multiple adapters for the same chain
    pub allow_multiple_adapters: bool,
    
    /// Minimum health check success rate to stay enabled
    pub min_health_success_rate: f64,
    
    /// Maximum response time before marking degraded
    pub max_response_time: Duration,
    
    /// Whether to automatically re-enable recovered adapters
    pub auto_reenable: bool,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            health_check_interval: Duration::from_secs(60),      // 1 minute
            max_unhealthy_duration: Duration::from_secs(300),    // 5 minutes
            max_adapters_per_chain: 3,                          // Allow some redundancy
            allow_multiple_adapters: true,                      // Permissionless by default
            min_health_success_rate: 0.95,                      // 95% success rate required
            max_response_time: Duration::from_secs(5),          // 5 second response time limit
            auto_reenable: true,                                // Auto recover by default
        }
    }
}

impl RegistryConfig {
    /// Create a new registry configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a more permissive configuration
    pub fn permissive() -> Self {
        Self {
            health_check_interval: Duration::from_secs(120),     // 2 minutes
            max_unhealthy_duration: Duration::from_secs(600),    // 10 minutes
            max_adapters_per_chain: 10,                         // More redundancy
            allow_multiple_adapters: true,
            min_health_success_rate: 0.90,                      // 90% success rate
            max_response_time: Duration::from_secs(10),         // 10 second limit
            auto_reenable: true,
        }
    }

    /// Create a stricter configuration
    pub fn strict() -> Self {
        Self {
            health_check_interval: Duration::from_secs(30),      // 30 seconds
            max_unhealthy_duration: Duration::from_secs(150),    // 2.5 minutes
            max_adapters_per_chain: 2,                          // Minimal redundancy
            allow_multiple_adapters: true,
            min_health_success_rate: 0.99,                      // 99% success rate
            max_response_time: Duration::from_secs(2),          // 2 second limit
            auto_reenable: false,                               // Manual re-enable
        }
    }

    /// Builder pattern methods
    pub fn with_health_check_interval(mut self, interval: Duration) -> Self {
        self.health_check_interval = interval;
        self
    }

    pub fn with_max_unhealthy_duration(mut self, duration: Duration) -> Self {
        self.max_unhealthy_duration = duration;
        self
    }

    pub fn with_max_adapters(mut self, max: usize) -> Self {
        self.max_adapters_per_chain = max;
        self
    }

    pub fn with_multiple_adapters(mut self, allow: bool) -> Self {
        self.allow_multiple_adapters = allow;
        self
    }

    pub fn with_min_success_rate(mut self, rate: f64) -> Self {
        self.min_health_success_rate = rate;
        self
    }

    pub fn with_max_response_time(mut self, time: Duration) -> Self {
        self.max_response_time = time;
        self
    }

    pub fn with_auto_reenable(mut self, auto: bool) -> Self {
        self.auto_reenable = auto;
        self
    }
} 