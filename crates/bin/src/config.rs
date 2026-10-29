use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for {key}")]
    Invalid { key: &'static str },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub log_format: LogFormat,
    pub allow_private_targets: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_iter(std::env::vars())
    }

    pub fn from_iter(
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let vars: HashMap<String, String> = vars.into_iter().collect();
        let required = |key: &'static str| {
            vars.get(key)
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .ok_or(ConfigError::Missing(key))
        };

        let bind_addr = vars
            .get("BIND_ADDR")
            .map(String::as_str)
            .unwrap_or("127.0.0.1:8090")
            .parse()
            .map_err(|_| ConfigError::Invalid { key: "BIND_ADDR" })?;

        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr,
            log_format: if vars.get("LOG_FORMAT").map(String::as_str) == Some("json") {
                LogFormat::Json
            } else {
                LogFormat::Pretty
            },
            allow_private_targets: vars
                .get("HOOKRELAY_ALLOW_PRIVATE_TARGETS")
                .is_some_and(|value| value == "1"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn reads_required_values_and_defaults() {
        let cfg = Config::from_iter(vars(&[
            ("DATABASE_URL", "postgres://localhost/hookrelay"),
            ("REDIS_URL", "redis://localhost:6379"),
        ]))
        .unwrap();

        assert_eq!(cfg.bind_addr, "127.0.0.1:8090".parse().unwrap());
        assert!(!cfg.allow_private_targets);
        assert_eq!(cfg.log_format, LogFormat::Pretty);
    }

    #[test]
    fn rejects_missing_database_url() {
        let err = Config::from_iter(vars(&[("REDIS_URL", "redis://localhost:6379")])).unwrap_err();

        assert_eq!(err, ConfigError::Missing("DATABASE_URL"));
    }

    #[test]
    fn rejects_invalid_bind_addr() {
        let err = Config::from_iter(vars(&[
            ("DATABASE_URL", "postgres://localhost/hookrelay"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("BIND_ADDR", "not-an-address"),
        ]))
        .unwrap_err();

        assert!(matches!(err, ConfigError::Invalid { .. }));
    }
}
