//! A client for the JSON API
//!
//! ```no_run
//! # async fn example() -> Result<(), risco_api::client::Error> {
//! use risco_api::client::Client;
//! use risco_api::{ArmMode, ArmRequest};
//!
//! let client = Client::new("http://127.0.0.1:8080");
//! if !client.status().await?.armed {
//!     let request = ArmRequest {
//!         partition: Some(1),
//!         mode: ArmMode::Partial,
//!     };
//!     client.arm(request).await?;
//! }
//! # Ok(())
//! # }
//! ```

use std::fmt;

use reqwest::{RequestBuilder, StatusCode};

use crate::{ArmRequest, DisarmRequest, ErrorResponse, Status};

/// A client for the JSON API
///
/// Cloning a `Client` is cheap; all clones share a connection pool.
#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
}

impl Client {
    /// Create a client for the API at `base_url`, such as `http://127.0.0.1:8080`
    ///
    /// Only plain HTTP is supported.
    pub fn new(base_url: &str) -> Self {
        Self::with_http_client(reqwest::Client::new(), base_url)
    }

    /// Create a client that sends requests through the given HTTP client
    ///
    /// This can be used to configure timeouts, or default headers to authenticate to a proxy in
    /// front of the server.
    pub fn with_http_client(http: reqwest::Client, base_url: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// Query whether the alarm is armed
    pub async fn status(&self) -> Result<Status, Error> {
        self.send(self.http.get(self.url("status"))).await
    }

    /// Arm the alarm, returning the status afterwards
    pub async fn arm(&self, request: ArmRequest) -> Result<Status, Error> {
        self.send(self.http.post(self.url("arm")).json(&request))
            .await
    }

    /// Disarm the alarm, returning the status afterwards
    pub async fn disarm(&self, request: DisarmRequest) -> Result<Status, Error> {
        self.send(self.http.post(self.url("disarm")).json(&request))
            .await
    }

    async fn send(&self, request: RequestBuilder) -> Result<Status, Error> {
        let response = request.send().await?;
        if let Err(error) = response.error_for_status_ref() {
            let status = response.status();
            // Fall back to the HTTP error if the body isn't from the server (say, from a proxy)
            return Err(match response.json().await {
                Ok(response) => Error::Api { status, response },
                Err(_) => Error::Http(error),
            });
        }

        Ok(response.json().await?)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url)
    }
}

/// Errors returned by the [`Client`]
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The request failed, or the response could not be read
    Http(reqwest::Error),
    /// The server responded with an error
    Api {
        status: StatusCode,
        response: ErrorResponse,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(f, "HTTP request failed: {error}"),
            Self::Api { status, response } => {
                write!(f, "server responded with {status}: {}", response.error)
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::Api { .. } => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}
