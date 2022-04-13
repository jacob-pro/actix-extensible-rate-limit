use crate::backend::fixed_window::FixedWindowInput;
use actix_web::dev::ServiceRequest;
use std::future::{ready, Ready};
use std::net::IpAddr;
use std::time::Duration;

pub type CustomFn = Box<dyn Fn(&ServiceRequest) -> Result<String, actix_web::Error>>;

/// Utility to create a simple input function for a
/// [FixedWindowBackend](crate::backend::fixed_window::FixedWindowBackend).
///
/// This will not be of any use if you want to use dynamic interval/request policies
/// or perform an asynchronous option; you should instead write your own input function.
pub struct SimpleInputFunctionBuilder {
    interval: Duration,
    max_requests: u64,
    real_ip_key: bool,
    path_key: bool,
    custom_key: Option<String>,
    custom_fn: Option<CustomFn>,
}

impl SimpleInputFunctionBuilder {
    pub fn new(interval: Duration, max_requests: u64) -> Self {
        Self {
            interval,
            max_requests,
            real_ip_key: false,
            path_key: false,
            custom_key: None,
            custom_fn: None,
        }
    }

    /// Adds the client's IP to the rate limiting key.
    ///
    /// # Security
    ///
    /// This uses
    /// [ConnectionInfo::realip_remote_addr()](actix_web::dev::ConnectionInfo::realip_remote_addr)
    /// internally which is only suitable for applications deployed behind a proxy.
    pub fn real_ip_key(mut self) -> Self {
        self.real_ip_key = true;
        self
    }

    /// Add the request path to the rate limiting key
    pub fn path_key(mut self) -> Self {
        self.path_key = true;
        self
    }

    /// Add a custom component to the rate limiting key
    pub fn custom_key(mut self, key: &str) -> Self {
        self.custom_key = Some(key.to_owned());
        self
    }

    /// Dynamically add a custom component to the rate limiting key
    pub fn custom_fn<F>(mut self, f: F) -> Self
    where
        F: Fn(&ServiceRequest) -> Result<String, actix_web::Error> + 'static,
    {
        self.custom_fn = Some(Box::new(f));
        self
    }

    pub fn build(
        self,
    ) -> impl Fn(&ServiceRequest) -> Ready<Result<FixedWindowInput, actix_web::Error>> + 'static
    {
        move |req| {
            ready((|| {
                let mut components = Vec::new();
                if let Some(prefix) = &self.custom_key {
                    components.push(prefix.clone());
                }
                if self.real_ip_key {
                    let info = req.connection_info();
                    components.push(ip_key(info.realip_remote_addr().unwrap()))
                }
                if self.path_key {
                    components.push(req.path().to_owned());
                }
                if let Some(f) = &self.custom_fn {
                    components.push(f(req)?)
                }
                let key = components.join("/");

                Ok(FixedWindowInput {
                    interval: self.interval,
                    max_requests: self.max_requests,
                    key,
                })
            })())
        }
    }
}

fn ip_key(ip_str: &str) -> String {
    let ip = ip_str
        .parse::<IpAddr>()
        .expect("Unable to parse remote IP address - proxy misconfiguration?");
    // TODO: Group ipv6 addresses
    ip.to_string()
}

#[cfg(test)]
mod tests {
    use crate::backend::fixed_window::{InMemoryBackend, SimpleInputFunctionBuilder};
    use crate::RateLimiter;
    use actix_web::App;
    use std::time::Duration;

    #[actix_web::test]
    async fn test_use_with_middleware() {
        // Check that all the type signatures work together
        let backend = InMemoryBackend::builder().build();
        let input_fn = SimpleInputFunctionBuilder::new(Duration::from_secs(60), 60).build();
        let limiter = RateLimiter::builder(backend, input_fn).build();
        actix_web::test::init_service(App::new().wrap(limiter)).await;
    }
}
